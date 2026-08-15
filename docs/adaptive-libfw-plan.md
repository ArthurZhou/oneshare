# Adaptive libfw — 规划文档 (2026-08-15)

状态：**规划中，未实现**。目标：按网络状况自动调 concurrency/window/chunkSize，
按客户端性能选压缩级别；服务端 config 只声明能力范围，选值由客户端决定。

## 现状（已确认，libfw-core/server 0.3.2）

- 压缩 = zrip（zstd via `zrip` crate，级别 -8..=4），但 `compressor()` 写死
  `ZRIP_DEFAULT_LEVEL = 1`，级别维度未暴露。
- 下载协商：client 发 `Accept-Encoding: zrip`，server 仅按 format 决定是否压缩，
  无级别协商（`negotiate_download_format`）。
- 上传：client 发 `x-libfw-compress`，server decompress —— zstd 解压与级别无关，
  **上传侧选级别零服务端成本**。
- concurrency / window / chunkSize 是纯客户端调度概念，服务端无感知。
- oneshare `/config.js` 目前下发固定值（compress: bool, concurrency, windows…）。

## 设计

### 1. 服务端只声明能力（oneshare 仓库）

`[libfw]` 变为范围声明，`/config.js` 下发 capabilities：

```json
{
  "compression": { "formats": ["zrip"], "levels": [-8,-4,0,1,4], "defaultLevel": 1 },
  "concurrency": { "min": 1, "max": 16, "default": 4 },
  "uploadWindow":   { "min": 1, "max": 8, "default": 4 },
  "downloadWindow": { "min": 1, "max": 8, "default": 4 },
  "chunkSize": { "min": 262144, "max": 8388608, "default": 2097152 },
  "maxRetries": { "min": 1, "max": 10, "default": 3 },
  "timeoutMs": { "min": 30000, "max": 1800000, "default": 600000 }
}
```

改动文件：`src/config.rs`（范围字段 + serde 默认）、`src/api/mod.rs`（广告 JSON）、
`config.example.toml`。

### 2. 引擎加 level 通道（libfw/wfw 仓库 → publish 0.3.3）

- `libfw-core`：`compressor_with_level(format, level)`（`ZripCompressor::new(level)`
  已 public，包装一行）。
- `libfw-server`：下载读新 header `x-libfw-compress-level`，校验 ∈ 服务端允许集，
  否则拒绝/就近 clamp；`ServerState` 加 `allowed_compress_levels: Option<Vec<i32>>`
  （None = 兼容旧行为，只用 default level）。
- `libfw-client` WASM：`compress: bool` → `compressLevel: i32 | null`；
  下载带 level header，上传按 level 压缩；导出 `probe_compress(bytes, level)`
  供 JS 实测（返回 {ratio, ms}）。

### 3. 客户端自适应（oneshare frontend/js/libfw.js）

- 探测：首传前一个 1 MiB Range GET → RTT（TTFB）+ 带宽；之后每次传输前用
  滑动平均（EWMA）刷新。
- 选参：在飞字节 ≈ concurrency × window × chunkSize，目标 = BDP（RTT × 带宽）；
  高延迟链路加大 chunkSize 减往返；乘积 clamp 进广告范围。
- 压缩级别：
  1. 扩展名黑名单（视频/音频/zip/图片等已压缩格式）→ identity，跳过；
  2. 否则拿文件真实 256 KiB 样本跑 2-3 个候选 level，按「省字节 / CPU 时间」
     选优；
  3. `navigator.hardwareConcurrency` 粗调：≤2 核封顶 level 1，≥8 核允许高级别；
  4. 带宽充足 + CPU 弱 → identity。
- 时机：每次传输前重算（引擎按实例固定参数，参数变化时重建 client，初始化 ~ms 级）。

## 注意点

- 本机 wfw checkout 是 0.1.x 旧版，引擎改动需先拉最新（remote URL 含 PAT，可直接拉）。
- oneshare `_enqueue` 串行化传输 → 并发只在单文件内生效；自适应主调 window、
  concurrency 次之。
- 服务端防御：level 必须服务端校验（客户端不可信）；concurrency/window 无需
  服务端强制。

## 待定决策

- [ ] 引擎部分（libfw crate 0.3.3 + npm）由谁改：Yitian 自己 vs 本机拉最新一起改
- [ ] 兼容策略：`/config.js` 直接换新 shape（前后端同仓发布，无兼容问题）
- [ ] 探测开销 & 时机：首传前阻塞 ~1s 探测是否可接受，或后台预热