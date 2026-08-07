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

/// Serves the frontend bootstrap config: `window.ONESHARE_BASE` tells the
/// browser what URL prefix the app is mounted under behind a reverse proxy, so
/// the client can build correct absolute URLs (e.g. `/oneshare/api/me`).
pub async fn config_js(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let base = state.config.base_url();
    let body = format!(
        "// OneShare bootstrap config\nwindow.ONESHARE_BASE = {};\n",
        serde_json::to_string(&base).unwrap_or_else(|_| "\"\"".to_string())
    );
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        body,
    )
}
