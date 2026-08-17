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
use crate::libtoken::{CodecPathValidator, OneshareTokenVerifier};
use axum::{
    body::Body,
    http::{HeaderValue, Request, Response, StatusCode, Uri},
    routing::{any_service, delete, get, post, put},
    Router,
};
use libfw_core::pathmap::PathCodec;
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
    /// libfw's encrypted path codec: real storage paths ↔ opaque `v1.…`
    /// shadow paths. The token endpoint uses it to bind tokens to shadows
    /// (never real paths) and to decode shadow inputs from the client.
    pub path_codec: Arc<dyn PathCodec>,
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

/// Filter that drops libfw's own upload-session temps from `/dir` listings.
///
/// An interrupted upload leaves a `.libfw-sess-*` temp (and its `.blocks`
/// sidecar) behind; a folder download must never pull a half-written temp
/// in. With `EncryptedPathCodec` the listed paths are opaque `v1.…` shadows,
/// so they can no longer be recognized by name after encoding — this filter
/// decodes each entry back to its real path and drops `.libfw-*` names.
/// Everything else (including the shadow paths) passes through untouched.
#[derive(Clone)]
struct DirListingFilter<S> {
    inner: S,
    codec: Arc<dyn PathCodec>,
}

impl<S> DirListingFilter<S> {
    fn new(inner: S, codec: Arc<dyn PathCodec>) -> Self {
        Self { inner, codec }
    }
}

impl<S, B> Service<Request<B>> for DirListingFilter<S>
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

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let codec = self.codec.clone();
        let fut = self.inner.call(req);
        Box::pin(async move {
            let response = fut.await.expect("inner service is infallible");
            Ok(filter_dir_listing(response, codec).await)
        })
    }
}

/// Drop `.libfw-*` entries (upload-session temps and their `.blocks`
/// sidecars) from a successful JSON `/dir` listing. Paths in the body are
/// shadows, so real names are only reachable via the codec.
async fn filter_dir_listing(response: Response<Body>, codec: Arc<dyn PathCodec>) -> Response<Body> {
    use axum::body::to_bytes;
    use axum::http::header;

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
    let limit: usize = 64 * 1024 * 1024;
    let bytes = match to_bytes(body, limit).await {
        Ok(b) => b,
        Err(_) => {
            tracing::warn!(
                "filter_dir_listing: listing body exceeded {} bytes; returning 500",
                limit
            );
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .unwrap();
        }
    };
    let mut value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };

    if let serde_json::Value::Array(entries) = &mut value {
        let mut kept: Vec<serde_json::Value> = Vec::with_capacity(entries.len());
        for entry in entries.drain(..) {
            let Some(serde_json::Value::String(shadow)) = entry.get("path") else {
                kept.push(entry);
                continue;
            };
            // Undecodable entries (shouldn't happen) are kept as-is; a
            // tampered shadow would fail decode and be dropped below by the
            // basename check only if it happens to decode.
            let real = codec.decode(shadow).unwrap_or_default();
            if real
                .rsplit('/')
                .next()
                .unwrap_or("")
                .starts_with(".libfw-")
            {
                continue;
            }
            kept.push(entry);
        }
        *entries = kept;
    }

    // The body changed, so any old Content-Length is stale.
    let mut parts = parts;
    parts.headers.remove(header::CONTENT_LENGTH);
    let body = Body::from(serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec()));
    Response::from_parts(parts, body)
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
    // libfw's EncryptedPathCodec: real storage paths become opaque `v1.…`
    // shadows everywhere they touch the browser — bearer tokens, `/file` and
    // `/dir` URLs, directory listings, upload echoes. The same codec drives
    // the embedded server (decode/encode) and the token endpoint (binding
    // tokens to shadows). Refuse to start without a valid key: an identity
    // fallback would silently leak real paths, which is what the codec
    // exists to prevent.
    let codec = config
        .libfw
        .path_codec()
        .unwrap_or_else(|e| panic!("invalid libfw path codec config: {e}"));

    let libfw_state = Arc::new(
        ServerState::builder()
            .storage(FsStorage::new(config.root_dir()))
            .verifier(OneshareTokenVerifier {
                hmac_key: hmac_secret.clone(),
            })
            .validator(CodecPathValidator::new(Arc::new(codec.clone())))
            .path_codec(codec.clone())
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
        path_codec: Arc::new(codec),
    });

    // libfw 0.3.4 ships a built-in stale session-temp sweeper
    // (`spawn_stale_session_cleanup`): the concurrent upload protocol leaves a
    // `.libfw-sess-*` temp (plus a `.blocks` sidecar) behind whenever a
    // browser dies mid-upload, and the sweeper removes ones whose last write
    // is older than the TTL — never committed user files. Defaults: 1h sweep
    // interval, 24h TTL.
    libfw_state.spawn_stale_session_cleanup();

    let libfw_app = libfw_router(libfw_state);

    // libfw file transfer routes (uses its own state, embedded via any_service).
    // FreshPathParams clears the outer router's `{*path}` captures so libfw's
    // own path match is the only one its `Path` extractor sees. There is no
    // path translation layer anymore: clients send opaque `v1.…` shadows
    // (from `/api/files/token` / `/dir` listings), and the embedded server
    // decodes + authorizes them itself — real paths never appear in URLs or
    // responses.
    let file_service = FreshPathParams::new(libfw_app.clone());
    let dir_service = DirListingFilter::new(
        FreshPathParams::new(libfw_app.clone()),
        state.path_codec.clone(),
    );
    // libfw's capability advertisement (`GET /capabilities`) is deliberately
    // public — the browser SDK fetches it before any auth to auto-tune. It has
    // no `{*path}` capture, so it needs no FreshPathParams.
    let caps_service = libfw_app;

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
        .route("/api/files/names", get(api::files::get_names))
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
        .route("/capabilities", any_service(caps_service))
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
