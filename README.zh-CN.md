# OneShare

一个自托管的网页文件共享应用 —— 类似 alist，但它只服务你选择共享的本地文件夹，并且不会把真实文件系统路径暴露给浏览器。

**功能特性**
- 📂 响应式网页界面浏览和管理本地目录
- ⬆⬇ 基于 [libfw](https://github.com/ArthurZhou/libfw) 的高速上传与下载 —— HTTP 流式传输、字节区间（Range）下载、上传断点续传、真实（服务端确认的）进度
- ✏️ 移动（拖拽或菜单）、重命名、删除、新建文件夹
- 🗑️ 可选的回收站目录 —— 删除的文件/文件夹会移动到那里（保留相对路径）而不是被永久删除
- 🔐 OIDC 登录（Authentik、Keycloak、Google、Microsoft Entra ID 等），并校验 ID Token 签名（JWKS）
- 🔒 基于 ACL 的访问控制 —— 按路径、按用户、按分组；默认拒绝（fail-closed）
- 👥 用户分组简化权限管理，另含保留的 `default`（默认）与 `guest`（访客）分组
- 🌐 Samba 风格的虚拟根目录 —— 共享文件夹以顶层共享形式呈现；真实路径永远不会到达非管理员浏览器
- ⚙️ 管理面板管理 ACL、分组与用户
- 🔀 可选 URL 前缀（`base_url`），可挂载在反向代理的共享域名子路径下

**架构**
- **后端** —— Rust：Axum、[libfw-server](https://github.com/ArthurZhou/libfw)、SQLite（rusqlite）、openidconnect
- **前端** —— 原生 JavaScript（无框架），使用 Vite/esbuild 压缩构建
- **传输** —— libfw WASM SDK 通过纯 HTTP（`/file`、`/dir`）与内嵌的 libfw 服务通信，支持并行字节区间下载与分块上传

---

## 快速开始

### 1. 配置

```bash
cp config.example.toml config.toml
```

编辑 `config.toml`：
- `[oidc]` —— 你的 OIDC 提供商的 `issuer_url`、`client_id`、`client_secret`、`redirect_uri`。
- `[server] hmac_secret` —— 一个强随机值（**≥ 16 字节**；否则服务器拒绝启动）。
- `[libfw] path_key` —— 64 位十六进制 AES-256 密钥：`openssl rand -hex 32`。

### 2. 构建并运行

前端是 `frontend/` 下的原生 JS。Debug 构建直接从磁盘提供；Release 构建会将其压缩到 `../static` 并内嵌进二进制。`pnpm build` 还会把 libfw WASM SDK 重新打包进 `frontend/vendor/`（debug 模式下从磁盘提供），所以 `pnpm install` 之后先执行一次。

```bash
# 一次性前端构建（把 JS/CSS 压缩进 ../static，并生成 frontend/vendor/ ——
# debug 模式从磁盘提供的 libfw SDK）
cd frontend && pnpm install && pnpm build
cd ..

# Debug —— 从磁盘提供 ./frontend
cargo run

# Release —— 把压缩后的前端内嵌进二进制
cd frontend && pnpm build
cd ..
cargo run --release
```

构建完全静态的 musl 二进制（GitHub Release 工作流所产出的形式）：

```bash
rustup target add x86_64-unknown-linux-musl            # 一次即可
sudo apt-get install -y musl-tools perl                # 一次即可（ring 需要 C 工具链 + perl）
CC=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl
```

### 3. 访问

浏览器打开 `http://localhost:3456`。**第一个登录的用户会自动成为管理员。**

---

## 配置

所有设置都在 `config.toml` 中。`config.example.toml` 是带完整注释的模板。

### `[server]`

| 键 | 默认值 | 说明 |
|-----|---------|-------------|
| `listen_addr` | `0.0.0.0` | 监听地址 |
| `listen_port` | `3456` | HTTP 端口 |
| `root_dir` | `./data` | 提供服务的根目录 |
| `database_url` | `oneshare.db` | SQLite 数据库路径 |
| `hmac_secret` | *（必填）* | 用于签署 libfw Bearer Token；必须 ≥ 16 字节且不能是默认值 |
| `base_url` | `""` | 反向代理下的 URL 前缀，如 `/oneshare`；为空表示域名根路径 |
| `session_cookie_secure` | `false` | 会话 Cookie 标记 `Secure`（HTTPS 部署时设为 `true`） |
| `allowed_origins` | `[]` | CORS 白名单；为空表示仅同源（推荐） |
| `trash_dir` | `""` | 设置后删除的文件/文件夹会**移动**到这里（保留相对路径）而不是永久删除；为空表示永久删除 |

### `[oidc]`

| 键 | 默认值 | 说明 |
|-----|---------|-------------|
| `issuer_url` | *（必填）* | OIDC Issuer URL |
| `client_id` / `client_secret` | *（必填）* | OIDC 客户端凭证 |
| `redirect_uri` | `http://localhost:3456/auth/callback` | 回调地址（设置 `base_url` 时需包含前缀） |
| `authorization_endpoint` / `token_endpoint` / `userinfo_endpoint` / `jwks_uri` | *（自动）* | 可选端点覆盖；当**全部**设置时会跳过 `.well-known` 发现（可离线启动），并用 `jwks_uri` 校验 ID Token |

### `[libfw]`

| 键 | 默认值 | 说明 |
|-----|---------|-------------|
| `path_key` | *（必填）* | 64 位十六进制 AES-256 密钥，用于把路径加密成不透明 shadow（`openssl rand -hex 32`） |
| `compression` | `"none"` | 下载压缩：`none` 或 `zrip` |
| `max_upload_size` | `107374182400` | 单次上传体上限（字节） |
| `concurrency` | `4` | 并行传输请求数 |
| `chunk_size` | `2097152` | 上传分块与下载字节区间大小 |
| `upload_window` / `download_window` | `4` | 单文件在途分块数（进度粒度 vs 吞吐量） |
| `max_retries` / `base_retry_delay_ms` / `max_retry_delay_ms` | `3` / `500` / `30000` | 重试策略 |
| `timeout_ms` | `600000` | 单次读取空闲超时 —— 保持宽松；任一读取卡住会中止整个传输 |
| `auto_tune` / `tune_ttl_ms` | `false` / `3600000` | 通过 `/capabilities` 自适应调优 |

---

## 反向代理与 URL 前缀

要把 OneShare 挂载到共享域名的子路径下，设置 `base_url` 并让代理转发**完整**路径（**不要**去掉前缀）：

```toml
[server]
base_url = "/oneshare"

[oidc]
redirect_uri = "https://example.com/oneshare/auth/callback"
```

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

`base_url` 为空（或 `/`）时应用在域名根路径提供服务。

---

## 身份认证

OneShare 使用 **OIDC**（OpenID Connect）登录。每次登录时服务器都会校验 ID Token 的签名（通过提供商的 JWKS，RS256）以及 `iss`/`aud`/`exp`/`nonce`。用户首次登录时自动创建。

- **角色 → 分组自动加入**：如果提供商的 `userinfo` 带有 `role` 声明，且其值与某个已有分组名一致，该用户会被自动加入该分组（从不移除）。
- **第一个登录的用户自动成为管理员。**

---

## ACL 权限模型

- **Read（读）** —— 列出目录、下载文件
- **Write（写）** —— 上传、新建文件夹、重命名、移动、删除
- **Admin（管理）** —— 完全控制；管理员用户绕过所有 ACL
- **Fail-closed（默认拒绝）** —— 没有匹配 ACL 的路径对任何人都不可访问。ACL 只授予访问权，绝不会“默认可读”。

ACL 条目可以针对某个用户或某个分组，并且是**路径前缀**匹配：对 `/photos` 设置的条目同样适用于其所有子目录。

### `default` 分组

所有未被分配到任何显式分组的用户，都会被视为保留的 **`default`** 分组成员 —— 一条 ACL（如 `/public → default（读）`）即可为所有未分组用户授予基础权限。被分配到显式分组的用户只使用这些分组，**不会**继承 `default`。该分组不可删除。

### `guest` 分组

所有**未登录**访问者都会被视作保留的 **`guest`** 分组成员 —— 一条 ACL（如 `/public → guest（读）`）即可把该文件夹设为公开。访客永远不是管理员，也永远看不到管理界面；未登录用户直接浏览 `guest` 授予的内容（页头仍保留登录链接）。该分组不可删除。

### Samba 风格的网页根目录

对非管理员用户，浏览器根路径（`/`）是一个**虚拟共享根目录**：用户可读的每个配置了 ACL 的目录，都会以顶层文件夹的形式出现，文件夹名取该目录的**叶子**名称。对 `/public（r）`、`/private（rw）` 和 `/nested/public2` 配置 ACL 后，用户会看到三个文件夹 —— `public`、`private`、`public2` —— 每个共享之上的层级都会被隐藏。地址栏反映当前文件夹（如 `#/public2/sub`）。

对非管理员用户，真实文件系统路径**永远不会发送到前端**：服务器在内部解析虚拟路径，ACL 校验始终基于真实路径。管理员**不受**虚拟根目录影响 —— 他们浏览真实文件系统树，以便在真实路径上配置 ACL。

---

## API

| 方法 | 路径 | 认证 | 说明 |
|--------|------|------|-------------|
| GET | `/api/me` | 会话 | 当前用户信息（访客返回 `{"user": null}`） |
| GET | `/api/files/list?path=` | 会话 | 目录列表 |
| GET | `/api/files/token?path=&op=` | 会话 | 签发 libfw Bearer Token（`op=read`\|`write`） |
| GET | `/api/files/names?paths=` | 会话 | 把不透明 shadow 路径解析为显示名称 |
| DELETE | `/api/files/delete?path=` | 会话 | 删除（或移入回收站） |
| PUT | `/api/files/rename?path=&new_name=` | 会话 | 重命名 |
| PUT | `/api/files/move?source=&destination=` | 会话 | 移动 |
| POST | `/api/files/mkdir?path=&name=` | 会话 | 新建文件夹 |
| GET | `/api/admin/users` | 管理员 | 列出用户 |
| GET/POST | `/api/admin/groups` | 管理员 | 列出 / 创建分组 |
| DELETE | `/api/admin/groups/{id}` | 管理员 | 删除分组 |
| GET | `/api/admin/groups/{id}/members` | 管理员 | 列出分组成员 |
| POST | `/api/admin/groups/add-user` / `remove-user` | 管理员 | 管理成员 |
| GET/POST/DELETE | `/api/admin/acl` | 管理员 | 管理 ACL |
| GET | `/config.js` | 公开 | 前端引导配置（base URL、libfw 选项、版本号） |
| GET | `/file/{*path}` | Bearer Token | 下载（libfw；支持 Range/ETag） |
| POST | `/file/{*path}` | Bearer Token | 上传（libfw） |
| GET | `/dir/{*path}` | Bearer Token | 目录列表（libfw） |
| GET | `/capabilities` | 公开 | libfw 能力公告（自适应调优） |
| GET | `/auth/login` / `/auth/callback` · POST `/auth/logout` | — | OIDC 流程 |

---

## 开发

- **构建前端：** `cd frontend && pnpm build` —— 把 `js/*` 与 `css/` 压缩进 `../static`，并把 libfw SDK 重新打包进 `static/vendor/`。
- **运行测试：** `cargo test`。
- **Release 构建：** `cargo build --release` —— 内嵌压缩后的前端；静态资源 URL 带内容哈希 `?v=` 做缓存破坏，并以 `immutable` 方式缓存。

---

## 文档

- [README.md](README.md) — English readme
- [docs/README.md](docs/README.md) — 中文文档索引
- [docs/configuration.md](docs/configuration.md) — 配置详解
- [docs/deployment.md](docs/deployment.md) — 部署与反向代理
- [docs/acl.md](docs/acl.md) — 权限模型
- [docs/development.md](docs/development.md) — 开发与构建
