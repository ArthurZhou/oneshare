mod acl;
mod api;
mod audit;
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
/// can be pruned instead of growing without bound. The `redirect_uri` used at
/// login time is kept so the callback exchanges the code against the same
/// redirect URI the provider was pointed at (it is derived from the request
/// URL the user actually used, not from config).
pub struct OidcPending {
    pub nonce: String,
    pub created: Instant,
    pub redirect_uri: String,
}

/// How long a generated OIDC state stays valid before it is pruned.
pub const OIDC_STATE_TTL: Duration = Duration::from_secs(10 * 60);

pub struct AppState {
    pub db: Arc<Database>,
    pub config: Config,
    pub oidc_client: OidcClient,
    pub oidc_states: Mutex<HashMap<String, OidcPending>>,
    pub session_manager: SessionManager,
    pub hmac_key: String,
    /// Whether the session cookie should be marked `Secure` (HTTPS-only).
    pub secure_cookies: bool,
    /// Rolling window of recent `/auth/login` timestamps, used to rate-limit
    /// login starts (the OIDC state map is an in-memory DoS surface).
    pub login_throttle: Mutex<std::collections::VecDeque<Instant>>,
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
    // Large listings (tens of thousands of entries) can exceed the old 64 MiB
    // cap and fail the whole directory with a 500. 256 MiB handles ~200k
    // entries; beyond that a 500 is still the honest answer rather than
    // serving a listing that would silently drop filtered temp entries.
    let limit: usize = 256 * 1024 * 1024;
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

/// Rewrite libfw's public `/capabilities` advertisement so the values
/// configured in `[libfw]` are used EXACTLY by the browser SDK's tuning
/// engine.
///
/// The tuning engine starts its ramp at the advertised `min`, so with the
/// stock advertisement it overrides the operator's `concurrency`/
/// `uploadWindow`/`downloadWindow`/`chunkSize` (e.g. it starts at concurrency
/// 1 instead of the configured 4, which the user observed as the config "not
/// taking effect"). Advertising each knob as a pinned range
/// (`min == default == max == configured`) makes the engine use precisely the
/// configured value, so the operator's settings actually apply.
#[derive(Clone)]
struct CapabilitiesRewrite<S> {
    inner: S,
    pin_concurrency: u32,
    pin_upload_window: u32,
    pin_download_window: u32,
    pin_chunk_size: u64,
}

impl<S> CapabilitiesRewrite<S> {
    fn new(inner: S, cfg: &crate::config::LibfwConfig) -> Self {
        Self {
            inner,
            pin_concurrency: cfg.concurrency.max(1),
            pin_upload_window: cfg.upload_window.max(1),
            pin_download_window: cfg.download_window.max(1),
            pin_chunk_size: cfg.chunk_size.max(1),
        }
    }
}

impl<S, B> Service<Request<B>> for CapabilitiesRewrite<S>
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
        let is_caps = req.uri().path() == "/capabilities";
        let pin_concurrency = self.pin_concurrency;
        let pin_upload_window = self.pin_upload_window;
        let pin_download_window = self.pin_download_window;
        let pin_chunk_size = self.pin_chunk_size;
        let fut = self.inner.call(req);
        Box::pin(async move {
            let resp = fut.await.expect("inner service is infallible");
            if is_caps && resp.status() == StatusCode::OK {
                return Ok(rewrite_capabilities(
                    resp,
                    pin_concurrency,
                    pin_upload_window,
                    pin_download_window,
                    pin_chunk_size,
                )
                .await);
            }
            Ok(resp)
        })
    }
}

/// Parse `/capabilities` JSON and pin the tuning knobs to the configured
/// `[libfw]` values. Always returns a response: the rewritten doc on success,
/// or the original passthrough when the body cannot be parsed/rewritten.
async fn rewrite_capabilities(
    resp: Response<Body>,
    pin_concurrency: u32,
    pin_upload_window: u32,
    pin_download_window: u32,
    pin_chunk_size: u64,
) -> Response<Body> {
    use axum::body::to_bytes;
    use axum::http::header;

    let (parts, body) = resp.into_parts();
    let passthrough = |bytes: Vec<u8>| Response::from_parts(parts.clone(), Body::from(bytes));

    // The capabilities doc is tiny; cap the read well above any real size.
    let Some(bytes) = to_bytes(body, 64 * 1024).await.ok() else {
        return Response::from_parts(parts, Body::empty());
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return passthrough(bytes.to_vec());
    };
    let Some(limits) = value.get_mut("limits") else {
        return passthrough(bytes.to_vec());
    };

    pin_knob(limits, "concurrency", pin_concurrency as i64);
    pin_knob(limits, "uploadWindow", pin_upload_window as i64);
    pin_knob(limits, "downloadWindow", pin_download_window as i64);
    pin_knob(limits, "chunkSize", pin_chunk_size as i64);

    let rewritten = serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rewritten))
        .unwrap_or_else(|_| passthrough(bytes.to_vec()))
}

/// Pin a `/capabilities` `limits.<key>` knob to the configured value by
/// advertising `min == default == max == value`, so libfw's tuning engine uses
/// exactly that value (it cannot ramp below or above it). This is what makes
/// the operator's `[libfw]` settings actually take effect instead of being
/// overridden by the auto-tuner's ramp.
///
/// Because all three fields are set to the same value the advertised range is
/// trivially consistent (`min <= default <= max`), so libfw's capabilities
/// parser never sees the "min > max" inversion.
fn pin_knob(limits: &mut serde_json::Value, key: &str, value: i64) {
    let Some(item) = limits.as_object_mut().and_then(|o| o.get_mut(key)) else {
        return;
    };
    let Some(o) = item.as_object_mut() else {
        return;
    };
    o.insert("min".to_string(), serde_json::json!(value));
    o.insert("default".to_string(), serde_json::json!(value));
    o.insert("max".to_string(), serde_json::json!(value));
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

    let db = Arc::new(Database::new(
        config.database_url(),
        config.server.admin_user.clone(),
    ).expect("Failed to initialize database"));

    // Audit-log retention: prune old entries right away so a fresh start
    // enforces the configured window even if the server was down for a while
    // (retention_days == 0 keeps everything).
    match db.prune_audit(config.server.audit_retention_days) {
        Ok(n) if n > 0 => tracing::info!(
            "Audit log: pruned {n} entries older than {} days",
            config.server.audit_retention_days
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("Audit log pruning failed: {e}"),
    }

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
        login_throttle: Mutex::new(std::collections::VecDeque::new()),
        path_codec: Arc::new(codec),
    });

    // Periodic cleanup of expired sessions (rows with expires_at in the past
    // are never removed otherwise, so the sessions table would grow forever).
    // The same hourly tick also prunes audit entries beyond the retention
    // window (`[server] audit_retention_days`).
    {
        let db = state.db.clone();
        let retention_days = state.config.server.audit_retention_days;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            loop {
                tick.tick().await;
                match db.delete_expired_sessions() {
                    Ok(n) => {
                        if n > 0 {
                            tracing::info!("Expired session cleanup removed {n} rows");
                        }
                    }
                    Err(e) => tracing::warn!("Expired session cleanup failed: {e}"),
                }
                match db.prune_audit(retention_days) {
                    Ok(n) => {
                        if n > 0 {
                            tracing::info!(
                                "Audit log cleanup removed {n} entries older than {retention_days} days"
                            );
                        }
                    }
                    Err(e) => tracing::warn!("Audit log cleanup failed: {e}"),
                }
            }
        });
    }

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
    // no `{*path}` capture, so it needs no FreshPathParams. We wrap it to raise
    // the advertised tuning minimums to the configured `[libfw]` values, so the
    // SDK's adaptive tuning never ramps below the operator's chosen settings.
    let caps_service = CapabilitiesRewrite::new(libfw_app, &config.libfw);

    let mut app = Router::new()
        .route("/auth/login", get(api::auth::login))
        .route("/auth/callback", get(api::auth::callback))
        .route("/auth/logout", post(api::auth::logout))
        .route("/api/me", get(api::auth::me))
        .route("/api/files/list", get(api::files::list))
        .route("/api/files/delete", delete(api::files::delete))
        .route("/api/files/rename", put(api::files::rename))
        .route("/api/files/move", put(api::files::mv))
        .route("/api/files/mkdir", post(api::files::mkdir))
        .route("/api/files/token", get(api::files::get_token))
        .route("/api/files/names", get(api::files::get_names))
        // Inline preview / online text editing. The PUT body is JSON-wrapped
        // text, so raise axum's 2 MiB default body limit on this route to
        // cover the 1 MiB text limit with worst-case JSON escaping.
        .route(
            "/api/files/content",
            get(api::files::get_content)
                .put(api::files::put_content)
                .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        // Inline binary preview (images/video/audio) for the file detail view.
        .route("/api/files/raw", get(api::files::raw))
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
        .route("/api/admin/audit", get(api::admin::list_audit))
        .route("/api/admin/audit", delete(api::admin::clear_audit))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `/capabilities`-shaped response with the limits we
    /// rewrite, mirroring libfw-server's advertised min/max.
    fn caps_response() -> Response<Body> {
        // Note: uploadWindow's default (8) exceeds the operator-configured max
        // (4) — the exact inconsistency that left the tuning engine stuck in
        // "uninitialized". The rewrite must clamp it back into [min, max].
        let json = r#"{
            "protocol": "libfw/1",
            "limits": {
                "concurrency":        {"min": 1, "max": 16, "default": 4},
                "uploadWindow":       {"min": 1, "max": 8, "default": 8},
                "downloadWindow":     {"min": 1, "max": 8, "default": 4},
                "chunkSize":          {"min": 262144, "max": 8388608, "default": 2097152},
                "downloadChunkSize":  {"min": 65536, "max": 4194304, "default": 262144}
            }
        }"#;
        Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap()
    }

    /// The rewrite pins each knob to the configured value
    /// (`min == default == max`), so the engine uses exactly the operator's
    /// settings, and never inverts a range (which libfw's capabilities parser
    /// panics on).
    #[tokio::test]
    async fn capabilities_rewrite_pins_configured_values() {
        let resp = rewrite_capabilities(
            caps_response(),
            4,               // concurrency
            4,               // uploadWindow
            4,               // downloadWindow
            8 * 1024 * 1024, // chunkSize = 8 MiB
        )
        .await;

        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let limits = &v["limits"];

        for key in [
            "concurrency",
            "uploadWindow",
            "downloadWindow",
            "chunkSize",
            "downloadChunkSize",
        ] {
            let mn = limits[key]["min"].as_i64().unwrap_or(i64::MAX);
            let mx = limits[key]["max"].as_i64().unwrap_or(1);
            assert!(
                mn <= mx,
                "limits.{key} inverted: min {mn} > max {mx} after rewrite"
            );
        }

        // Every listed knob is pinned: min == default == max == configured.
        for (key, value) in [
            ("concurrency", 4i64),
            ("uploadWindow", 4i64),
            ("downloadWindow", 4i64),
            ("chunkSize", 8 * 1024 * 1024i64),
        ] {
            assert_eq!(limits[key]["min"].as_i64(), Some(value), "{key}.min");
            assert_eq!(limits[key]["default"].as_i64(), Some(value), "{key}.default");
            assert_eq!(limits[key]["max"].as_i64(), Some(value), "{key}.max");
        }

        // The (unlisted) download chunk keeps libfw's own defaults untouched.
        assert_eq!(limits["downloadChunkSize"]["max"].as_i64(), Some(4 * 1024 * 1024));
        assert_eq!(limits["downloadChunkSize"]["min"].as_i64(), Some(65536));
        assert_eq!(limits["downloadChunkSize"]["default"].as_i64(), Some(262144));
    }

    /// Pinning to a value below libfw's advertised minimum is still safe: all
    /// three fields become that value, so the range stays consistent
    /// (`min == default == max`) and never inverts.
    #[tokio::test]
    async fn capabilities_rewrite_pins_small_values() {
        let resp = rewrite_capabilities(caps_response(), 1, 1, 1, 1024).await;
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let limits = &v["limits"];

        for key in [
            "concurrency",
            "uploadWindow",
            "downloadWindow",
            "chunkSize",
            "downloadChunkSize",
        ] {
            let mn = limits[key]["min"].as_i64().unwrap_or(i64::MAX);
            let mx = limits[key]["max"].as_i64().unwrap_or(1);
            assert!(mn <= mx, "limits.{key} inverted after pin");
        }
        // chunkSize is pinned to 1024 across min/default/max (no clamping).
        assert_eq!(limits["chunkSize"]["min"].as_i64(), Some(1024));
        assert_eq!(limits["chunkSize"]["default"].as_i64(), Some(1024));
        assert_eq!(limits["chunkSize"]["max"].as_i64(), Some(1024));
    }
}
