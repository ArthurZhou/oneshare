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
    });
    // `window.ONESHARE_TRASH` tells the frontend whether deletions are moved
    // to the configured trash directory (instead of being permanently
    // deleted), so the UI can word its delete confirmation accordingly.
    let trash_json = serde_json::json!({
        "enabled": state.config.trash_path().is_some(),
    });
    let body = format!(
        "// OneShare bootstrap config\nwindow.ONESHARE_BASE = {};\nwindow.ONESHARE_LIBFW = {};\nwindow.ONESHARE_TRASH = {};\n",
        serde_json::to_string(&base).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(&libfw_json).unwrap_or_else(|_| "{}".to_string()),
        serde_json::to_string(&trash_json).unwrap_or_else(|_| "{\"enabled\":false}".to_string())
    );
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        body,
    )
}
