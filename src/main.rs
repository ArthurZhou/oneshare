mod acl;
mod api;
mod auth;
mod config;
mod db;
mod libtoken;
mod models;
mod statics;

use crate::auth::oidc::OidcClient;
use crate::auth::session::SessionManager;
use crate::config::Config;
use crate::db::Database;
use crate::libtoken::OneshareTokenVerifier;
use axum::{
    body::Body,
    http::{HeaderValue, Request, Response, StatusCode, Uri},
    routing::{any_service, delete, get, post, put},
    Router,
};
use libfw_core::auth::PathValidator;
use libfw_server::{router as libfw_router, FsStorage, ServerState};
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tower::{Layer, Service, ServiceBuilder};
use tower_http::cors::{Any, CorsLayer};

/// A pending OIDC login. The CSRF `state` maps to a nonce (validated on the
/// callback for replay/tamper protection) and a creation time so stale states
/// can be pruned instead of growing without bound.
pub struct OidcPending {
    pub nonce: String,
    pub created: Instant,
}

/// How long a generated OIDC state stays valid before it is pruned.
pub const OIDC_STATE_TTL: Duration = Duration::from_secs(10 * 60);

pub struct AppState {
    pub db: Database,
    pub config: Config,
    pub oidc_client: OidcClient,
    pub oidc_states: Mutex<HashMap<String, OidcPending>>,
    pub session_manager: SessionManager,
    pub hmac_key: String,
    /// Whether the session cookie should be marked `Secure` (HTTPS-only).
    pub secure_cookies: bool,
}

/// Forward a request to an inner service with a fresh (empty) extension map.
///
/// The embedded libfw router defines its own `/file/{*path}` and `/dir/{*path}`
/// routes. When it is mounted with `any_service` under those same patterns, both
/// the outer and inner routers capture `{*path}`; axum's `Path` extractor inside
/// libfw then sees two captures ("Expected 1 but got 2") and answers every
/// upload/download with 500. Captured path params live in the request extensions,
/// so dropping them before delegation leaves libfw's own match as the only one.
#[derive(Clone)]
struct FreshPathParams<S> {
    inner: S,
}

impl<S> FreshPathParams<S> {
    fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S, B> Service<axum::http::Request<B>> for FreshPathParams<S>
where
    S: Service<axum::http::Request<B>> + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: axum::http::Request<B>) -> Self::Future {
        let (mut parts, body) = req.into_parts();
        // Fresh extensions drop the outer router's captured path params.
        parts.extensions = axum::http::Extensions::new();
        self.inner.call(axum::http::Request::from_parts(parts, body))
    }
}

/// Translates the Samba-style virtual paths on `/file/*` and `/dir/*` into the
/// real paths the embedded libfw router serves, so non-admin users never send
/// (or receive) real filesystem paths. ACLs are still evaluated on real paths
/// server-side.
///
/// The bearer token's `sub` identifies the user; their ACL shares resolve the
/// virtual request path. The token itself is bound to the real path (issued by
/// `/api/files/token`), so libfw's own path-permission check still gates the
/// real path afterwards. Admin users are unaffected — they browse the real
/// tree, so their paths pass through unchanged.
#[derive(Clone)]
struct VirtualTranslate<S> {
    inner: S,
    state: Arc<AppState>,
}

impl<S> VirtualTranslate<S> {
    fn new(inner: S, state: Arc<AppState>) -> Self {
        Self { inner, state }
    }
}

impl<S, B> Service<Request<B>> for VirtualTranslate<S>
where
    S: Service<Request<B>, Response = Response<Body>, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let path = req.uri().path().to_owned();
        let prefix = path
            .strip_prefix("/file/")
            .map(|rest| ("/file/", rest))
            .or_else(|| path.strip_prefix("/dir/").map(|rest| ("/dir/", rest)));

        let Some((pfx, virt)) = prefix else {
            return Box::pin(self.inner.call(req));
        };

        // Browsers always advertise `Accept-Encoding: gzip, deflate, br, zstd`,
        // and libfw maps `zstd` to its proprietary zrip framing. Only the libfw
        // SDK (which explicitly sends `Accept-Encoding: zrip` and decompresses)
        // can consume zrip; a plain browser fetch would save the compressed
        // bytes as the file itself. So for `/file` downloads, force identity
        // unless the client explicitly asked for zrip.
        if pfx == "/file/"
            && matches!(req.method(), &axum::http::Method::GET | &axum::http::Method::HEAD)
        {
            let wants_zrip = req
                .headers()
                .get(axum::http::header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(|v| {
                    v.split(',')
                        .any(|t| t.trim().eq_ignore_ascii_case("zrip"))
                })
                .unwrap_or(false);
            if !wants_zrip {
                req.headers_mut().insert(
                    axum::http::header::ACCEPT_ENCODING,
                    axum::http::header::HeaderValue::from_static("identity"),
                );
            }
        }

        let token = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.to_string());

        let Some(token) = token else {
            // No token: let libfw answer 401/403 itself.
            return Box::pin(self.inner.call(req));
        };

        // The frontend percent-encodes path segments (e.g. `%2F` for the slash
        // inside `private/name`), so the captured `virt` may still be encoded.
        // Decode it before resolving, and re-encode the real path afterwards.
        use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
        let virt_decoded = percent_decode_str(virt)
            .decode_utf8_lossy()
            .into_owned();

        // Compute the rewritten path (None = leave unchanged). For `/dir`
        // requests from non-admins we also record the real→display mapping so
        // the listing response can be rewritten back to display paths (the
        // browser SDK walks `/dir` and re-fetches each listed path, so they
        // must stay virtual).
        let mut dir_map: Option<(String, String)> = None; // (real_prefix, display_prefix)
        let rewrite: Option<String> = match crate::libtoken::verify_token(&self.state.hmac_key, &token)
        {
            Err(_) => None, // invalid/expired: let libfw answer 401
            Ok(payload) => {
                let uid: i64 = match payload.sub.parse() {
                    Ok(uid) => uid,
                    Err(_) => return Box::pin(self.inner.call(req)),
                };
                let Some(user) = self.state.db.get_user_by_id(uid).ok().flatten() else {
                    return Box::pin(self.inner.call(req));
                };
                let Some(groups) = self.state.db.get_effective_groups(uid).ok() else {
                    return Box::pin(self.inner.call(req));
                };
                let Some(entries) = self.state.db.list_acl_entries().ok() else {
                    return Box::pin(self.inner.call(req));
                };
                if crate::api::files::sees_real_tree(&user, &groups, &entries) {
                    // Admin / root-ACL holder browses the real tree: the path is
                    // already real, no translation.
                    None
                } else {
                    let shares = crate::acl::user_shares(&user, &groups, &entries);
                    match crate::acl::resolve_virtual(&virt_decoded, &shares) {
                        Some(real) => {
                            // Encode each segment (keeping `/` as separator) so
                            // the rewritten URI survives axum's Path decode and
                            // non-ASCII names round-trip correctly.
                            let encoded = real
                                .split('/')
                                .map(|seg| utf8_percent_encode(seg, NON_ALPHANUMERIC).to_string())
                                .collect::<Vec<_>>()
                                .join("/");
                            if pfx == "/dir/" {
                                dir_map = Some((real.clone(), virt_decoded.clone()));
                            }
                            Some(format!("{}{}", pfx, encoded))
                        }
                        None => {
                            // Not a reachable share: 404 instead of letting a
                            // virtual path reach libfw/FsStorage.
                            return Box::pin(async {
                                Ok(Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Body::empty())
                                    .unwrap())
                            });
                        }
                    }
                }
            }
        };

        if let Some(new_path) = rewrite {
            let uri = req.uri().clone();
            let mut parts = uri.clone().into_parts();
            let path_and_query = match uri.path_and_query() {
                Some(pq) => match pq.query() {
                    Some(q) if !q.is_empty() => format!("{new_path}?{q}"),
                    _ => new_path.clone(),
                },
                None => new_path.clone(),
            };
            parts.path_and_query = Some(
                path_and_query
                    .parse()
                    .expect("rewritten path is a valid path-and-query"),
            );
            *req.uri_mut() = Uri::from_parts(parts).expect("valid rewritten uri");
        }

        match dir_map {
            Some((real, disp)) => {
                // Non-admin `/dir`: forward, then rewrite the listing body so
                // real filesystem paths never reach the browser.
                let fut = Box::pin(self.inner.call(req));
                Box::pin(async move {
                    let resp = fut.await.expect("inner service is infallible");
                    Ok(rewrite_dir_response(resp, &real, &disp).await)
                })
            }
            None => Box::pin(self.inner.call(req)),
        }
    }
}

/// Rewrite the JSON directory-listing body returned by libfw so every entry
/// `path` maps from the REAL prefix (under `root_dir`) back to the user's
/// DISPLAY (virtual) path.
///
/// libfw's `FsStorage::list_dir` returns full paths relative to the mount
/// root (e.g. `nested/public2/a.txt`), and the browser SDK walks `/dir`
/// recursively and re-fetches each listed path through `/file/..` /
/// `/dir/..`. Those follow-up requests must use virtual paths, so we map
/// `nested/public2/a.txt` → `public2/a.txt` here, keeping real paths out of
/// the browser exactly like every other API response.
async fn rewrite_dir_response(
    response: Response<Body>,
    real_prefix: &str,
    display_prefix: &str,
) -> Response<Body> {
    use axum::body::to_bytes;
    use axum::http::header;

    // Only successful JSON listings are rewritten.
    if response.status() != StatusCode::OK {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("json"))
        .unwrap_or(false);
    if !is_json {
        return response;
    }

    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 4 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };
    let mut value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };

    let real = real_prefix.trim_matches('/');
    let disp = display_prefix.trim_matches('/');
    if let serde_json::Value::Array(entries) = &mut value {
        for entry in entries.iter_mut() {
            let Some(obj) = entry.as_object_mut() else { continue };
            let Some(serde_json::Value::String(p)) = obj.get_mut("path") else {
                continue;
            };
            let p = std::mem::take(p);
            obj.insert(
                "path".to_string(),
                serde_json::Value::String(map_listing_path(&p, real, disp)),
            );
        }
    }

    // The body changed, so any old Content-Length is stale.
    let mut parts = parts;
    parts.headers.remove(header::CONTENT_LENGTH);
    let body = Body::from(serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec()));
    Response::from_parts(parts, body)
}

/// Map a real listing path (e.g. `nested/public2/a.txt`) back to the display
/// path the user sees (e.g. `public2/a.txt`) using the real/display prefixes
/// of the current `/dir` request.
fn map_listing_path(real_path: &str, real_prefix: &str, display_prefix: &str) -> String {
    let suffix = if real_prefix.is_empty() {
        real_path.to_string()
    } else {
        match real_path.strip_prefix(real_prefix) {
            Some(rest) => rest.trim_start_matches('/').to_string(),
            None => real_path.to_string(),
        }
    };
    if display_prefix.is_empty() {
        suffix
    } else {
        format!("{}/{}", display_prefix, suffix)
    }
}

/// Strips a URL prefix from incoming request paths before forwarding to the
/// inner router, and answers 404 for paths outside the prefix so the rest of
/// the domain (other apps behind the same reverse proxy) is untouched.
///
/// This is used instead of `Router::nest` because axum's `nest` cannot route
/// the nested router's frontend fallback for the bare prefix path (`/prefix`
/// and `/prefix/`): matchit's `{*rest}` requires at least one segment, so the
/// static frontend at the app root would 404. Stripping the path keeps every
/// route, the libfw `/file` and `/dir` transfer endpoints, and the ServeDir
/// fallback working exactly as they do at the domain root. An empty prefix is
/// a no-op (the app is served at the domain root).
#[derive(Clone)]
struct PrefixStrip<S> {
    inner: S,
    prefix: String,
    prefix_with_slash: String,
}

impl<S> Service<Request<Body>> for PrefixStrip<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        // No prefix configured: pass through unchanged (domain-root serving).
        if self.prefix.is_empty() {
            return Box::pin(self.inner.call(req));
        }

        let path = req.uri().path().to_owned();
        let query = req
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();

        // The bare prefix path must redirect to the trailing-slash form.
        // index.html loads its assets with RELATIVE paths (css/style.css,
        // js/*.js, config.js), so from "/oneshare" the browser would resolve
        // them against the domain root and 404. "/oneshare/" keeps them under
        // the prefix where they actually exist.
        if path == self.prefix {
            let location = format!("{}{}", self.prefix_with_slash, query);
            return Box::pin(async move {
                Ok(Response::builder()
                    .status(StatusCode::PERMANENT_REDIRECT)
                    .header(axum::http::header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap())
            });
        }

        let stripped = if let Some(rest) = path.strip_prefix(&self.prefix_with_slash) {
            // "/oneshare/css/style.css" -> "/css/style.css"
            Some(format!("/{rest}"))
        } else {
            // Outside the prefix: leave it for the rest of the domain.
            None
        };

        let stripped = match stripped {
            Some(p) => p,
            None => {
                return Box::pin(async {
                    Ok(Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap())
                });
            }
        };

        // Rewrite the request URI (path + query) so axum routes on the
        // unprefixed path.
        let uri = req.uri().clone();
        let mut parts = uri.clone().into_parts();
        let path_and_query = match uri.path_and_query() {
            Some(pq) => match pq.query() {
                Some(q) if !q.is_empty() => format!("{stripped}?{q}"),
                _ => stripped.clone(),
            },
            None => stripped.clone(),
        };
        parts.path_and_query = Some(
            path_and_query
                .parse()
                .expect("stripped path is a valid path-and-query"),
        );
        *req.uri_mut() = Uri::from_parts(parts).expect("valid stripped uri");

        Box::pin(self.inner.call(req))
    }
}

#[derive(Clone)]
struct PrefixStripLayer {
    prefix: String,
    prefix_with_slash: String,
}

impl PrefixStripLayer {
    fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
            prefix_with_slash: format!("{prefix}/"),
        }
    }
}

impl<S> Layer<S> for PrefixStripLayer {
    type Service = PrefixStrip<S>;
    fn layer(&self, inner: S) -> Self::Service {
        PrefixStrip {
            inner,
            prefix: self.prefix.clone(),
            prefix_with_slash: self.prefix_with_slash.clone(),
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oneshare=info,libfw_server=info".into()),
        )
        .init();

    let config = Config::from_file("config.toml").expect("Failed to load config");

    // Refuse to start with an empty, too-short, or default `hmac_secret`: it
    // signs libfw bearer tokens that grant file access, so a weak default lets
    // anyone forge a token and read/write every file.
    let raw_hmac = config.hmac_secret();
    let trimmed = raw_hmac.trim();
    if trimmed.is_empty()
        || trimmed == "change-me-to-a-random-32-byte-secret"
        || trimmed.len() < 16
    {
        panic!(
            "Refusing to start: [server] hmac_secret must be set to a strong random value \
             (>= 16 bytes, not the default) in config.toml before running."
        );
    }

    std::fs::create_dir_all(config.root_dir()).expect("Failed to create root dir");

    let db = Database::new(config.database_url()).expect("Failed to initialize database");

    let oidc_client = crate::auth::oidc::OidcClient::new(&config.oidc)
        .await
        .expect("Failed to initialize OIDC client");

    let session_manager = SessionManager::new();

    let hmac_secret = config.hmac_secret().to_string();

    // ── libfw file-transfer layer ──
    // The compression and upload-size knobs come from `[libfw]` in config.toml.
    // The browser SDK (libfw-client) decompresses zrip streams itself, so
    // enabling compression is safe now; the frontend learns whether the server
    // serves zrip via config.js (window.ONESHARE_LIBFW.compress) and sets its
    // own `compress` flag to match.
    let libfw_state = Arc::new(
        ServerState::builder()
            .storage(FsStorage::new(config.root_dir()))
            .verifier(OneshareTokenVerifier {
                hmac_key: hmac_secret.clone(),
            })
            .validator(PathValidator::new())
            .compression(config.libfw.compression_format())
            .max_upload_size(config.libfw.max_upload_size)
            .build(),
    );

    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        oidc_client,
        oidc_states: Mutex::new(HashMap::new()),
        session_manager,
        hmac_key: hmac_secret,
        secure_cookies: config.server.session_cookie_secure,
    });

    let libfw_app = libfw_router(libfw_state);

    // libfw file transfer routes (uses its own state, embedded via any_service).
    // FreshPathParams clears the outer router's `{*path}` captures so libfw's
    // own path match is the only one its `Path` extractor sees. VirtualTranslate
    // rewrites non-admin virtual paths (/file/public2/... → /file/nested/public2/...)
    // so real paths never leave the server; admin paths pass through.
    let file_service = VirtualTranslate::new(FreshPathParams::new(libfw_app.clone()), state.clone());
    let dir_service = VirtualTranslate::new(FreshPathParams::new(libfw_app), state.clone());

    let mut app = Router::new()
        .route("/auth/login", get(api::auth::login))
        .route("/auth/callback", get(api::auth::callback))
        .route("/auth/logout", get(api::auth::logout))
        .route("/api/me", get(api::auth::me))
        .route("/api/files/list", get(api::files::list))
        .route("/api/files/delete", delete(api::files::delete))
        .route("/api/files/rename", put(api::files::rename))
        .route("/api/files/move", put(api::files::mv))
        .route("/api/files/mkdir", post(api::files::mkdir))
        .route("/api/files/token", get(api::files::get_token))
        .route("/api/admin/users", get(api::admin::list_users))
        .route("/api/admin/groups", get(api::admin::list_groups))
        .route("/api/admin/groups", post(api::admin::create_group))
        .route("/api/admin/groups/{id}", delete(api::admin::delete_group))
        .route("/api/admin/groups/{id}/members", get(api::admin::list_group_members))
        .route("/api/admin/groups/add-user", post(api::admin::add_user_to_group))
        .route("/api/admin/groups/remove-user", post(api::admin::remove_user_from_group))
        .route("/api/admin/acl", get(api::admin::list_acl))
        .route("/api/admin/acl", post(api::admin::set_acl))
        .route("/api/admin/acl/{id}", delete(api::admin::remove_acl))
        // Frontend bootstrap config: tells the browser what URL prefix the app
        // is mounted under (window.ONESHARE_BASE), so the client can build
        // absolute URLs when served behind a reverse proxy on a shared domain.
        .route("/config.js", get(api::config_js))
        .route("/file/{*path}", any_service(file_service))
        .route("/dir/{*path}", any_service(dir_service))
        // Frontend fallback: debug builds serve the loose ./frontend directory
        // (with revalidation so browsers never cache stale JS/CSS during dev);
        // release builds serve the minified assets embedded by build.rs. See
        // src/statics.rs.
        .fallback_service(crate::statics::frontend_router())
        .with_state(state);

    // CORS: only allow explicitly-configured origins ([server] allowed_origins).
    // When empty (the default) no CORS headers are emitted and cross-origin
    // browser requests are blocked, which is correct for the same-origin
    // frontend. Never permissive.
    if !config.server.allowed_origins.is_empty() {
        let origins: Vec<HeaderValue> = config
            .server
            .allowed_origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        app = app.layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    // Support running behind a reverse proxy on a shared domain: when a URL
    // prefix is configured (e.g. base_url = "/oneshare"), strip it from every
    // request before routing. This keeps all routes, the libfw /file and /dir
    // transfer endpoints, and the static frontend fallback working exactly as
    // they do at the domain root — including the bare prefix path, which axum's
    // `nest` cannot route to the frontend fallback. Requests outside the prefix
    // get 404 so other apps on the same domain are untouched. With an empty
    // prefix the middleware is a no-op (domain-root serving).
    let base = config.base_url();
    let app = ServiceBuilder::new()
        .layer(PrefixStripLayer::new(&base))
        .service(app);

    let addr = format!("{}:{}", config.listen_addr(), config.listen_port());
    tracing::info!("OneShare starting on http://{}", addr);
    tracing::info!("OneShare URL prefix (base path): {:?}", base);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, tower::make::Shared::new(app))
        .await
        .unwrap();
}
