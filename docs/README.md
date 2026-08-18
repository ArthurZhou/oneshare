# OneShare 文档（中文）

欢迎使用 OneShare —— 一个自托管的网页文件共享应用。本目录提供中文文档。

## 快速入口

- 想要**上手运行**：先看根目录的 [README](../README.md) 或 [README.zh-CN.md](../README.zh-CN.md) 的「快速开始」。
- 想要**配置每一项**：见 [配置详解](configuration.md)。
- 想要**部署到公网 / 反向代理**：见 [部署与反向代理](deployment.md)。
- 想要**搞清权限体系**：见 [权限模型](acl.md)。
- 想要**参与开发 / 自定义构建**：见 [开发与构建](development.md)。

## 项目速览

| 项 | 说明 |
|----|------|
| 后端 | Rust（Axum + libfw-server + SQLite + openidconnect） |
| 前端 | 原生 JavaScript，Vite/esbuild 压缩构建 |
| 传输 | [libfw](https://github.com/ArthurZhou/libfw) WASM SDK ↔ 内嵌 libfw 服务（HTTP，并行 Range 下载、分块上传、断点续传、服务端确认进度） |
| 认证 | OIDC（校验 ID Token 签名，JWKS） |
| 授权 | 按路径 / 用户 / 分组的 ACL，默认拒绝 |
| 数据 | 配置文件 `config.toml`；数据库 SQLite；被共享目录为 `root_dir` |

## 文档目录

- [配置详解](configuration.md) —— 每个配置项的说明与示例
- [部署与反向代理](deployment.md) —— 反向代理、URL 前缀、HTTPS、升级、资源缓存
- [权限模型](acl.md) —— ACL、`default`/`guest` 分组、虚拟共享根目录、真实路径保护
- [开发与构建](development.md) —— 前端构建、测试、Release/musl 构建、常见开发要点
