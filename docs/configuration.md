# 配置详解

OneShare 的所有配置都在工作目录下的 `config.toml` 中。最稳妥的做法是从模板复制并逐项填写：

```bash
cp config.example.toml config.toml
```

`config.example.toml` 自带完整注释。下面按小节说明每一项。

---

## `[server]`

| 键 | 默认值 | 说明 |
|-----|---------|------|
| `listen_addr` | `0.0.0.0` | 监听地址。仅本机访问可填 `127.0.0.1`。 |
| `listen_port` | `3456` | HTTP 端口。 |
| `root_dir` | `./data` | 提供共享服务的根目录（相对路径基于运行目录）。 |
| `database_url` | `oneshare.db` | SQLite 数据库文件路径。用户、分组、ACL、会话都存这里。 |
| `hmac_secret` | *（必填）* | 用于签署 libfw Bearer Token 的密钥。**必须 ≥ 16 字节且不能是默认值**，否则服务器拒绝启动。 |
| `base_url` | `""` | 反向代理下的 URL 前缀（如 `/oneshare`）。为空表示域名根路径。见[部署与反向代理](deployment.md)。 |
| `session_cookie_secure` | `false` | 会话 Cookie 是否标记 `Secure`。通过 HTTPS 对外提供服务时设为 `true`，否则浏览器可能不保存 Cookie。 |
| `allowed_origins` | `[]` | CORS 允许的跨域来源列表。空数组表示仅同源（推荐）。 |
| `trash_dir` | `""` | 回收站目录。设置后，删除操作会把文件/文件夹**移动**到这里（保留相对路径，便于恢复），而不是永久删除；为空则永久删除。相对路径基于 `root_dir`，例如 `.trash`（点开头条目不会出现在列表里）。 |

### 回收站说明

- 回收站保留被删项目的**相对路径**（与操作系统回收站类似），方便还原。
- 发生重名时会自动追加 ` (1)`、` (2)` 之类的后缀。
- 跨设备（EXDEV）时会自动退化为“复制 + 删除”。
- 不提供自动清理策略；需要时手动清理该目录即可。

---

## `[oidc]`

| 键 | 默认值 | 说明 |
|-----|---------|------|
| `issuer_url` | *（必填）* | OIDC Provider 的 Issuer URL。 |
| `client_id` / `client_secret` | *（必填）* | 在 Provider 控制台注册的客户端凭证。 |
| `redirect_uri` | `http://localhost:3456/auth/callback` | 回调地址。设置了 `base_url` 时**必须包含前缀**，例如 `https://example.com/oneshare/auth/callback`。 |
| `authorization_endpoint` | *（自动发现）* | 可选覆盖授权端点。 |
| `token_endpoint` | *（自动发现）* | 可选覆盖令牌端点。 |
| `userinfo_endpoint` | *（自动发现）* | 可选覆盖用户信息端点。 |
| `jwks_uri` | *（自动发现）* | 可选覆盖 JWKS 端点，用于校验 ID Token 签名。 |

**离线启动**：当上面四个端点（`authorization_endpoint`、`token_endpoint`、`userinfo_endpoint`、`jwks_uri`）**全部**显式配置时，启动会跳过 `.well-known` 发现流程，可在无外网环境下启动（用于测试/内网部署）。

**安全提示**：服务器会在每次登录时用 JWKS 校验 ID Token 的签名（RS256）以及 `iss`/`aud`/`exp`/`nonce`，并校验状态与 nonce 防 CSRF / 重放。

---

## `[libfw]`

`[libfw]` 既配置内嵌 libfw 服务端（`compression`、`max_upload_size`），也配置浏览器端 SDK（其余项会通过 `/config.js` 下发）。

| 键 | 默认值 | 说明 |
|-----|---------|------|
| `path_key` | *（必填）* | 64 位十六进制 AES-256 密钥（`openssl rand -hex 32`）。用于把真实路径加密成不透明的 `v1.…` shadow，浏览器永远拿不到真实路径。**跨重启保持稳定**；更换会使未过期 Token 失效（它们 1 小时 TTL 内自然过期）。 |
| `compression` | `"none"` | 下载压缩格式：`none`（普通浏览器请求最安全）或 `zrip`（SDK 可解压）。前端 `compress` 标志会与此保持一致。 |
| `max_upload_size` | `107374182400`（100 GiB） | 单次上传请求体上限（字节）。 |
| `concurrency` | `4` | 并行传输请求数。 |
| `chunk_size` | `2097152`（2 MiB） | 上传分块大小，同时作为下载字节区间大小（前端会把 `downloadChunkSize` 设成此值）。 |
| `upload_window` | `4` | 单文件在途上传分块数。总在途 ≈ `concurrency × upload_window`。**值越小上传进度越平滑**（每个 wave 更小），但高延迟链路下吞吐会下降；低带宽链路下几乎无影响。 |
| `download_window` | `4` | 单文件在途下载字节区间数（下载侧镜像 `upload_window`）。 |
| `max_retries` | `3` | 单次传输重试次数。 |
| `base_retry_delay_ms` | `500` | 重试基础延迟（指数退避起点）。 |
| `max_retry_delay_ms` | `30000` | 重试最大延迟。 |
| `timeout_ms` | `600000`（10 分钟） | **单次读取**的空闲超时。libfw 引擎在任一次读取停滞超过该值时会中止整个传输 —— 它不是总时长上限。慢链路/大文件/高延迟下请保持宽松；设为 `0` 会禁用 JS 定时器（浏览器网络错误仍可发现断连）。 |
| `auto_tune` | `false` | 是否启用自适应调优：浏览器探测公开的 `/capabilities` 并基于真实传输统计动态调整并发/window/分块大小。默认关闭 = 完全按上述静态配置执行。 |
| `tune_ttl_ms` | `3600000` | 调优结果在单个来源上的复用时长（仅 `auto_tune = true` 时有意义）。 |

---

## 配置示例（最小可用）

```toml
[server]
listen_addr = "0.0.0.0"
listen_port = 3456
root_dir = "./data"
database_url = "oneshare.db"
hmac_secret = "改为一个至少16字节的强随机字符串"
base_url = ""
session_cookie_secure = false
allowed_origins = []
trash_dir = ".trash"

[oidc]
issuer_url = "https://auth.example.com"
client_id = "oneshare"
client_secret = "your-client-secret"
redirect_uri = "http://localhost:3456/auth/callback"

[libfw]
path_key = "用 openssl rand -hex 32 生成"
compression = "none"
max_upload_size = 107374182400
```

> 注意：`hmac_secret` 为空、过短（<16 字节）或等于默认值时，服务器会拒绝启动；`path_key` 不是合法 64 位十六进制时同样拒绝启动。
