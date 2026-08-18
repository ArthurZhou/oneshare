# 部署与反向代理

## 目录

1. [基础运行](#基础运行)
2. [URL 前缀与反向代理](#url-前缀与反向代理)
3. [HTTPS 与 Cookie](#https-与-cookie)
4. [升级与资源缓存](#升级与资源缓存)
5. [常见问题排查](#常见问题排查)

---

## 基础运行

```bash
# 1. 配置
cp config.example.toml config.toml
# 编辑 config.toml：OIDC、hmac_secret、path_key

# 2. 构建前端（一次性）
cd frontend && pnpm install && pnpm build && cd ..

# 3. 运行
cargo run --release
```

- 服务器从**运行目录**读取 `config.toml`，并把 `root_dir`、`database_url` 等相对路径基于该目录解析。
- 监听端口默认 `3456`，浏览器打开 `http://<host>:3456`。
- **第一个登录的用户自动成为管理员**。

### 只提供单个可执行文件

Release 构建会把压缩后的前端**内嵌进二进制**（`include_str!`/`include_bytes!`），部署时只需分发一个可执行文件 + `config.toml`（+ 数据目录/数据库）。

---

## URL 前缀与反向代理

要让 OneShare 与其他应用共享同一域名，可挂载在子路径下，例如 `https://example.com/oneshare/`。

1. `config.toml` 设置前缀：

```toml
[server]
base_url = "/oneshare"

[oidc]
# 回调地址必须包含前缀
redirect_uri = "https://example.com/oneshare/auth/callback"
```

2. 反向代理转发**完整路径**，**不要剥掉前缀**（`proxy_pass` 末尾不带 `/`）：

```nginx
location /oneshare/ {
    proxy_pass http://127.0.0.1:3456;     # 末尾不带斜杠：保留前缀
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    # 大文件上传/下载
    client_max_body_size 0;
    proxy_request_buffering off;
    proxy_buffering off;
}
```

> **务必关闭代理的请求体缓冲**（`proxy_request_buffering off;`、`proxy_buffering off;`、`client_max_body_size 0;`），否则大文件上传/下载可能被缓冲层打断或限制。

`base_url` 为空（或 `/`）时应用在域名根路径提供服务，无需额外配置。

---

## HTTPS 与 Cookie

- 前端由 OneShare 自身提供（无 CSP 外链），通过反向代理终结 TLS 即可。
- 通过 HTTPS 对外提供服务时，请设置 `session_cookie_secure = true`，否则浏览器（遵循 Secure 语义）可能拒绝保存会话 Cookie 导致登录失效。
- OIDC 的 `redirect_uri` 必须使用与对外访问一致的 **https** 地址，并在 Provider 控制台登记。

---

## 升级与资源缓存

**Release 构建**会把前端静态资源（JS/CSS/vendor wasm）以 `immutable` 长缓存策略对外提供，并用内容哈希（`?v=…`）做缓存破坏：

- `index.html` 永不缓存（`no-cache`），每次携带新的 `?v=` 版本号；
- JS/CSS/wasm 在同一个二进制生命周期内视为不可变，缓存一年；
- 升级后，浏览器首次访问会拿到带新版本号的 HTML → 自动拉取新资源，不会用到旧 JS/wasm。

因此**升级时只需替换二进制并重启**，无需用户手动清缓存。若个别浏览器仍出现异常，强制刷新（Ctrl+F5）即可。

构建产物归属：

```bash
cd frontend && pnpm build      # 生成 ../static（gitignored）与 frontend/vendor/
cargo build --release          # 内嵌 ../static
```

> 调试模式（`cargo run`）直接读磁盘上的 `./frontend`，并始终 `no-cache`，方便开发。

---

## 常见问题排查

| 现象 | 排查方向 |
|------|----------|
| 启动即退出，提示 `hmac_secret` / `path_key` | 见[配置详解](configuration.md)：`hmac_secret` ≥ 16 字节且非默认；`path_key` 为 64 位十六进制。 |
| 登录后 Cookie 不生效 | 通过 HTTPS 提供服务时未设 `session_cookie_secure = true`。 |
| 上传/下载被中断 | 反向代理缓冲未关闭（见上文 nginx 示例）；`timeout_ms` 过小（保持 10 分钟默认）。 |
| 升级后页面/功能异常 | 释放浏览器缓存（Ctrl+F5）；确认前后端版本一致（单一二进制部署不存在前后端版本漂移）。 |
| 看不到任何文件夹 | ACL 默认拒绝。请用管理员登录后到「管理」面板为对应路径配置 ACL（读/写），并确认是否用到 `default`/`guest` 分组。 |
| 端口/前缀改动后 404 | `base_url` 与反向代理 `proxy_pass` 前缀必须一致，且 OIDC `redirect_uri` 需同步。 |
