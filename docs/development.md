# 开发与构建

## 目录结构

```
├── Cargo.toml              # Rust 依赖
├── config.example.toml     # 带注释的配置模板
├── config.toml             # 本地配置（gitignore）
├── src/
│   ├── main.rs             # 启动、路由、AppState、libfw 挂载、URL 前缀剥离
│   ├── config.rs           # 配置解析与默认值
│   ├── models.rs           # 共享的数据结构 / 请求结构
│   ├── db.rs               # SQLite 存取（用户/分组/ACL/会话）
│   ├── acl.rs              # ACL 纯函数引擎（含单元测试）
│   ├── libtoken.rs         # libfw Bearer Token 签发/校验
│   ├── statics.rs          # 前端静态资源（debug 磁盘 / release 内嵌 + 缓存）
│   └── api/                # HTTP 处理器（files / admin / auth）
│   └── auth/               # OIDC 客户端与会话管理
├── frontend/
│   ├── index.html
│   ├── css/style.css       # 样式源
│   ├── js/*.js             # 原生 JS 源
│   ├── vendor/             # libfw WASM SDK（pnpm build 生成）
│   ├── vite.config.js
│   └── scripts/build-js.mjs
└── static/                 # 构建产物（gitignore；release 内嵌）
```

---

## 构建

### 前端

```bash
cd frontend
pnpm install
pnpm build
```

`pnpm build` 做两件事：
1. 用 Vite/esbuild 压缩 `js/*.js` 与 `css/`，输出到 `../static`；
2. 从 `node_modules/libfw-client` 重新打包 UMD 与 wasm 到 `static/vendor/`（同时生成 `frontend/vendor/` 供 debug 模式从磁盘提供）。

> 前端是**经典 `<script>` 标签**结构（共享全局变量如 `API`、`loadFiles`），无法用 Vite 打包成模块，因此采用“逐个文件 esbuild 压缩”的方式。Vite 只负责 HTML/CSS。

### 后端

```bash
# Debug —— 直接提供 ./frontend
cargo run

# Release —— 内嵌 ../static
cargo build --release
```

Release 构建通过 `src/statics.rs` 的 `include_str!`/`include_bytes!` 把 `../static` 内嵌进二进制；**改了前端后必须重新 `cargo build --release`** 才会生效。

### 完全静态的 musl 二进制（CI 产物）

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt-get install -y musl-tools perl     # ring 需要 C 工具链 + perl
CC=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl
```

---

## 测试

```bash
cargo test
```

- 单元测试主要在 `src/acl.rs`（权限引擎、虚拟路径解析、路径穿越防护、显示路径往返）与 `src/api/files.rs`（shadow 复合解码、回收站移动）。
- 均为纯逻辑/临时目录测试，无需外部服务。

### 手工端到端验证（不改动真实数据）

仓库记录了一套隔离的测试方法：建立 `.test-env/`（独立的 `config.toml`，显式配置四个 OIDC 端点以离线启动、独立的 `data/` 与 `test.db`、`frontend` Junction 指向真实 `../frontend`），用 Python sqlite3 直接种入 `users`/`sessions` 数据，然后浏览器携带 `fh_session=<id>` Cookie 访问。

---

## 关键设计点

- **URL 前缀**：通过 `PrefixStrip` 中间件在路由前剥离 `base_url` 前缀（不能用 `Router::nest`，因为 matchit 的 `{*rest}` 需要至少一个段，裸前缀会 404）。代理必须转发完整路径。
- **libfw 挂载**：libfw 的 `/file/{*path}`、`/dir/{*path}` 用 `FreshPathParams` 包装，清空外层路由捕获的路径参数，避免 axum `Path` 提取器“期望 1 个捕获却得到 2 个”。
- **虚拟路径翻译**：非管理员经 `resolve_virtual` 把显示路径映射为真实路径；`VirtualTranslate` 对 `/file`/`/dir` 做同样的翻译，保证真实路径不出网。`/dir` 列表响应还会把真实路径改写成显示路径。
- **路径加密**：`EncryptedPathCodec`（AES-256-GCM）把真实路径加密为 `v1.<base64url>` shadow；Token 绑定到 shadow，路径永不暴露给浏览器。
- **复合 shadow 解码**：上传的 plan 路径形如 `{目录shadow}/{相对路径}`，`/api/files/names` 用 `decode_compound`（最长可解码前缀 + 字面后缀）解析，与 libfw-server 的 `resolve_client_path` 行为一致。
- **静态资源缓存**：release 下 `index.html` 永不缓存，其余资源按内容哈希 `?v=` 版本化并 `immutable` 缓存一年；`config.js` 永不缓存（携带版本号）。
- **回收站**：删除 = 移动进 `trash_dir`（保留相对路径，重名加后缀）；跨设备自动退化为复制+删除。

---

## 常见开发提醒

- 新增前端资源（`js/*.js` 等）时，需要同步更新三处：`frontend/` 源文件 → `src/statics.rs` 的 `embedded_serve` 路由臂与 `include_str!` 静态（以及 `versioned_html` 的版本化列表）→ 重新构建 release。
- 改完 `config.rs` 默认值后，同步更新 `config.example.toml` 注释。
- 数据库 schema 变更集中在 `db.rs` 的迁移逻辑中。
