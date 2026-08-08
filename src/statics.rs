//! Frontend static asset serving.
//!
//! Mirrors the zline_sso strategy:
//!   - **Debug** builds serve the loose `./frontend` directory from disk so
//!     edits show up instantly (the original behavior — no rebuild needed).
//!   - **Release** builds serve the *minified* assets built by Vite
//!     (`cd frontend && pnpm build`, which writes to `../static`) and embedded
//!     into the binary via `include_str!`. There is no loose webpage to ship
//!     alongside the release binary.
//!
//! The dynamic `/config.js` bootstrap route lives in `api::config_js` and is
//! registered as a real route (before the fallback), so it is not embedded.

use axum::http::header;
use axum::Router;

/// Router used as the frontend fallback (`fallback_service`).
pub fn frontend_router() -> Router {
    #[cfg(debug_assertions)]
    {
        use tower::Layer;
        use tower_http::services::ServeDir;
        use tower_http::set_header::SetResponseHeaderLayer;

        // Debug: keep the existing strategy — serve ./frontend with
        // revalidation so browsers never cache stale JS/CSS during development.
        Router::new().fallback_service(
            SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache, must-revalidate"),
            )
            .layer(ServeDir::new("./frontend")),
        )
    }

    #[cfg(not(debug_assertions))]
    {
        Router::new().fallback(embedded_serve)
    }
}

/// Serves one of the minified, compile-time-embedded frontend assets.
#[cfg(not(debug_assertions))]
async fn embedded_serve(uri: axum::http::Uri) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let (body, content_type) = match uri.path() {
        "/" | "/index.html" => (Body::from(INDEX_HTML), "text/html; charset=utf-8"),
        "/css/main.css" => (Body::from(STYLE_CSS), "text/css; charset=utf-8"),
        "/js/api.js" => (Body::from(API_JS), "application/javascript; charset=utf-8"),
        "/js/auth.js" => (Body::from(AUTH_JS), "application/javascript; charset=utf-8"),
        "/js/libfw.js" => (Body::from(LIBFW_JS), "application/javascript; charset=utf-8"),
        "/js/file-explorer.js" => (Body::from(FILE_EXPLORER_JS), "application/javascript; charset=utf-8"),
        "/js/admin.js" => (Body::from(ADMIN_JS), "application/javascript; charset=utf-8"),
        "/js/app.js" => (Body::from(APP_JS), "application/javascript; charset=utf-8"),
        // Vendored libfw SDK (classic-script UMD bundle) + its WASM engine.
        "/vendor/libfw-client.js" => (Body::from(VENDOR_LIBFW_JS), "application/javascript; charset=utf-8"),
        "/vendor/libfw_client_bg.wasm" => (Body::from(VENDOR_LIBFW_WASM), "application/wasm"),
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };

    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache, must-revalidate"),
        ],
        body,
    )
        .into_response()
}

// Release-only: minified assets produced by `cd frontend && pnpm build` into
// ../static. cargo tracks `include_str!`/`include_bytes!` files and rebuilds
// when they change.
#[cfg(not(debug_assertions))]
static INDEX_HTML: &str = include_str!("../static/index.html");
#[cfg(not(debug_assertions))]
static STYLE_CSS: &str = include_str!("../static/css/main.css");
#[cfg(not(debug_assertions))]
static API_JS: &str = include_str!("../static/js/api.js");
#[cfg(not(debug_assertions))]
static AUTH_JS: &str = include_str!("../static/js/auth.js");
#[cfg(not(debug_assertions))]
static LIBFW_JS: &str = include_str!("../static/js/libfw.js");
#[cfg(not(debug_assertions))]
static FILE_EXPLORER_JS: &str = include_str!("../static/js/file-explorer.js");
#[cfg(not(debug_assertions))]
static ADMIN_JS: &str = include_str!("../static/js/admin.js");
#[cfg(not(debug_assertions))]
static APP_JS: &str = include_str!("../static/js/app.js");
#[cfg(not(debug_assertions))]
static VENDOR_LIBFW_JS: &str = include_str!("../static/vendor/libfw-client.js");
#[cfg(not(debug_assertions))]
static VENDOR_LIBFW_WASM: &[u8] = include_bytes!("../static/vendor/libfw_client_bg.wasm");
