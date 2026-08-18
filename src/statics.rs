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

/// Content-hash of the entire embedded frontend bundle. Stable for a given
/// binary and changes whenever any embedded asset changes, so it can be used
/// as a cache-busting version (`?v=…`) on asset URLs that are served with
/// long-lived `immutable` cache headers. This is what keeps those long cache
/// lives safe across upgrades: a new binary hashes differently, so the served
/// `index.html` (which is never cached) references fresh `?v=` URLs and the
/// browser never reuses a stale JS/CSS/wasm.
#[cfg(not(debug_assertions))]
pub fn static_version() -> &'static str {
    use std::sync::OnceLock;
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for bytes in [
            INDEX_HTML.as_bytes(),
            STYLE_CSS.as_bytes(),
            API_JS.as_bytes(),
            ICONS_JS.as_bytes(),
            AUTH_JS.as_bytes(),
            LIBFW_JS.as_bytes(),
            FILE_EXPLORER_JS.as_bytes(),
            ADMIN_JS.as_bytes(),
            APP_JS.as_bytes(),
            VENDOR_LIBFW_JS.as_bytes(),
            VENDOR_LIBFW_WASM,
        ] {
            h.update(bytes);
        }
        format!("{:x}", h.finalize())
    })
}

/// Debug builds serve the loose `./frontend` with `no-cache`; no versioning.
#[cfg(debug_assertions)]
pub fn static_version() -> &'static str {
    ""
}

/// Append `?v=<bundle version>` to every local asset reference in the served
/// `index.html`, so the long-lived `immutable` cache lives are keyed per
/// build. Only the known asset URLs are rewritten; `config.js` and the
/// `auth/login` route are left untouched (config.js is dynamic and never
/// cached).
#[cfg(not(debug_assertions))]
fn versioned_html(html: &str) -> String {
    let v = static_version();
    let mut out = html.to_string();
    for path in [
        "js/api.js",
        "js/icons.js",
        "js/auth.js",
        "js/libfw.js",
        "js/file-explorer.js",
        "js/admin.js",
        "js/app.js",
        "vendor/libfw-client.js",
        "./css/main.css",
    ] {
        out = out.replace(path, &format!("{path}?v={v}"));
    }
    out
}

/// One embedded asset, with its exact served body (zero-copy for the statics,
/// owned for the version-rewritten HTML), content type and strong ETag.
#[cfg(not(debug_assertions))]
struct EmbeddedAsset {
    body: axum::body::Bytes,
    etag: String,
    content_type: &'static str,
    html: bool,
}

/// Look up an embedded asset by request path, computed lazily once per
/// process (the ETag and versioned HTML never change for a given binary).
#[cfg(not(debug_assertions))]
fn asset_for(path: &str) -> Option<&'static EmbeddedAsset> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static ASSETS: OnceLock<HashMap<&'static str, EmbeddedAsset>> = OnceLock::new();
    let map = ASSETS.get_or_init(|| {
        use axum::body::Bytes;
        use sha2::{Digest, Sha256};
        let mut m = HashMap::new();
        let mut push = |path: &'static str,
                        bytes: &'static [u8],
                        content_type: &'static str,
                        html: bool| {
            let body = if html {
                Bytes::from(
                    versioned_html(std::str::from_utf8(bytes).unwrap_or_default()).into_bytes(),
                )
            } else {
                Bytes::from_static(bytes)
            };
            let etag = format!("\"{:x}\"", Sha256::digest(&body));
            m.insert(
                path,
                EmbeddedAsset {
                    body,
                    etag,
                    content_type,
                    html,
                },
            );
        };
        push("/", INDEX_HTML.as_bytes(), "text/html; charset=utf-8", true);
        push("/index.html", INDEX_HTML.as_bytes(), "text/html; charset=utf-8", true);
        push("/css/main.css", STYLE_CSS.as_bytes(), "text/css; charset=utf-8", false);
        push("/js/api.js", API_JS.as_bytes(), "application/javascript; charset=utf-8", false);
        push("/js/icons.js", ICONS_JS.as_bytes(), "application/javascript; charset=utf-8", false);
        push("/js/auth.js", AUTH_JS.as_bytes(), "application/javascript; charset=utf-8", false);
        push("/js/libfw.js", LIBFW_JS.as_bytes(), "application/javascript; charset=utf-8", false);
        push(
            "/js/file-explorer.js",
            FILE_EXPLORER_JS.as_bytes(),
            "application/javascript; charset=utf-8",
            false,
        );
        push("/js/admin.js", ADMIN_JS.as_bytes(), "application/javascript; charset=utf-8", false);
        push("/js/app.js", APP_JS.as_bytes(), "application/javascript; charset=utf-8", false);
        push(
            "/vendor/libfw-client.js",
            VENDOR_LIBFW_JS.as_bytes(),
            "application/javascript; charset=utf-8",
            false,
        );
        push(
            "/vendor/libfw_client_bg.wasm",
            VENDOR_LIBFW_WASM,
            "application/wasm",
            false,
        );
        m
    });
    map.get(path)
}

/// Serves one of the minified, compile-time-embedded frontend assets.
///
/// Caching policy (release):
/// - `index.html` is never cached (`no-cache, must-revalidate`): it carries
///   the `?v=` cache-busting version on every asset URL, so it must stay
///   fresh for upgrades to take effect on the next load.
/// - Every other asset (JS/CSS/vendor wasm) is immutable for the lifetime of
///   a binary, so it is served `public, max-age=31536000, immutable`, keyed
///   by the bundle version on its URL.
/// - A strong content ETag supports conditional GETs (304) on revalidation.
#[cfg(not(debug_assertions))]
async fn embedded_serve(
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let Some(asset) = asset_for(uri.path()) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };

    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(asset.etag.as_str())
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let cache_control = if asset.html {
        "no-cache, must-revalidate"
    } else {
        "public, max-age=31536000, immutable"
    };

    (
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::CACHE_CONTROL, cache_control),
            (header::ETAG, asset.etag.as_str()),
        ],
        Body::from(asset.body.clone()),
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
static ICONS_JS: &str = include_str!("../static/js/icons.js");
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
