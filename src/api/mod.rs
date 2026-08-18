pub mod auth;
pub mod files;
pub mod admin;

use axum::{
    extract::State,
    http::header,
    response::IntoResponse,
};
use std::sync::Arc;

use crate::AppState;

/// Serves the frontend bootstrap config:
/// - `window.ONESHARE_BASE` tells the browser what URL prefix the app is
///   mounted under behind a reverse proxy, so the client can build correct
///   absolute URLs (e.g. `/oneshare/api/me`).
/// - `window.ONESHARE_LIBFW` carries the client-side `libfw-client` SDK
///   options, read from `[libfw]` in config.toml, so the frontend configures
///   its libfw transfer client from the backend instead of hard-coding it.
///   `compress` mirrors `[libfw] compression` (zrip ⇔ true) so the SDK only
///   negotiates compression the server actually serves.
pub async fn config_js(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use libfw_core::compress::CompressionFormat;

    let base = state.config.base_url();
    let libfw = &state.config.libfw;
    let libfw_json = serde_json::json!({
        "compress": libfw.compression_format() == CompressionFormat::Zrip,
        "concurrency": libfw.concurrency,
        "chunkSize": libfw.chunk_size,
        "uploadWindow": libfw.upload_window,
        "downloadWindow": libfw.download_window,
        "maxRetries": libfw.max_retries,
        "baseRetryDelayMs": libfw.base_retry_delay_ms,
        "maxRetryDelayMs": libfw.max_retry_delay_ms,
        "timeoutMs": libfw.timeout_ms,
        "autoTune": libfw.auto_tune,
        "tuneTtlMs": libfw.tune_ttl_ms,
    });
    // `window.ONESHARE_TRASH` tells the frontend whether deletions are moved
    // to the configured trash directory (instead of being permanently
    // deleted), so the UI can word its delete confirmation accordingly.
    let trash_json = serde_json::json!({
        "enabled": state.config.trash_path().is_some(),
    });
    // `window.ONESHARE_VERSION` is the release bundle's content hash (see
    // `statics::static_version`): the frontend uses it to version the wasm URL
    // so release-mode `immutable` caching never serves a stale engine after an
    // upgrade. Empty in debug builds.
    let body = format!(
        "// OneShare bootstrap config\nwindow.ONESHARE_BASE = {};\nwindow.ONESHARE_LIBFW = {};\nwindow.ONESHARE_TRASH = {};\nwindow.ONESHARE_VERSION = {};\n",
        serde_json::to_string(&base).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(&libfw_json).unwrap_or_else(|_| "{}".to_string()),
        serde_json::to_string(&trash_json).unwrap_or_else(|_| "{\"enabled\":false}".to_string()),
        serde_json::to_string(crate::statics::static_version())
            .unwrap_or_else(|_| "\"\"".to_string())
    );
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            // Never cache config.js: it carries the version that busts the
            // long-lived asset caches, so it must always be fresh.
            (header::CACHE_CONTROL, "no-cache, must-revalidate"),
        ],
        body,
    )
}
