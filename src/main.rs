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
    response::IntoResponse,
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

/// Middleware for `/s/{token}/{*path}`: a share URL IS a temporary session.
///
/// Every request a share visitor makes is prefixed with `/s/{token}/`. This
/// service resolves the share, gets (or self-heals) the temporary identity's
/// standard session, **swaps the URL token for that session cookie**, and
/// rewrites the request onto the normal routes (`/api/…`, `/file/…`,
/// `/dir/…`, static assets). Downstream there is ZERO share-specific
/// authorization: the visitor is simply a temporary, read-only user flowing
/// through the unmodified session → user → ACL → virtual-share-root
/// pipeline, which is what makes libfw transfers work untouched.
#[derive(Clone)]
struct ShareProxy {
    inner: Router,
    db: Arc<Database>,
}

impl ShareProxy {
    fn new(inner: Router, db: Arc<Database>) -> Self {
        Self { inner, db }
    }
}

impl Service<Request<Body>> for ShareProxy {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Fully qualified: axum's Router implements Service for several body
        // types, so the method call alone is ambiguous here.
        <Router as Service<Request<Body>>>::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let db = self.db.clone();
        let mut inner = self.inner.clone();
        Box::pin(async move {
            // Path shape: /s/{token}/{rest…} (base_url already stripped).
            let path = req.uri().path().to_string();
            let mut segs = path.splitn(4, '/');
            let _root = segs.next(); // ""
            let s = segs.next().unwrap_or("");
            let token = segs.next().unwrap_or("");
            let rest = segs.next().unwrap_or("");
            if s != "s" || token.is_empty() {
                return Ok((StatusCode::NOT_FOUND, "Not found").into_response());
            }

            // Route allowlist: a share visitor is a READ-ONLY identity, so the
            // proxy only forwards the read-only file APIs, libfw transfers and
            // the app shell assets. Everything else (auth flows, admin
            // routes, share management, uploads/mkdir/delete) never reaches
            // the core router under a share URL — even though ACLs and the
            // admin checks would also reject them, keeping them out of the
            // proxy removes login/logout and context-confusion side effects.
            let first = rest.split('/').next().unwrap_or("");
            let allowed = match first {
                "api" => matches!(
                    rest.strip_prefix("api/").unwrap_or(""),
                    "me"
                        | "files/list"
                        | "files/token"
                        | "files/names"
                        | "files/raw"
                        | "files/content"
                        | "files/size"
                ),
                "file" | "dir" | "capabilities" | "config.js" | "js" | "css" | "vendor" => true,
                _ => false,
            };
            if !allowed {
                return Ok((StatusCode::NOT_FOUND, "Not available on a share link").into_response());
            }

            let Some(_) = db.get_share_link(token).ok().flatten() else {
                return Ok((
                    StatusCode::NOT_FOUND,
                    "Share not found or expired",
                )
                    .into_response());
            };

            // The share session is VIRTUAL: the injected cookie value is
            // `share:<token>` and no session row is ever created.
            // `get_user_from_cookie` resolves it against `share_links` on
            // every request, so expiry/revocation take effect immediately and
            // nothing is written to the users/groups/sessions tables — the
            // share token in the URL is the one and only credential.
            let session_id = format!(
                "{}{}",
                crate::auth::session::SHARE_COOKIE_PREFIX,
                token
            );

            // Rewrite `/s/{token}/{rest}` → `/{rest}` (query string kept for
            // the `?v=` cache busters), and drop the outer router's captured
            // path params so the inner router does its own matching (same
            // reason FreshPathParams exists for the libfw routes).
            let (mut parts, body) = req.into_parts();
            let pq = match parts.uri.query() {
                Some(q) if !rest.is_empty() => format!("/{}?{}", rest, q),
                Some(q) => format!("/?{}", q),
                None if !rest.is_empty() => format!("/{}", rest),
                None => "/".to_string(),
            };
            if let Ok(uri) = Uri::builder().path_and_query(pq).build() {
                parts.uri = uri;
            }
            parts.extensions = axum::http::Extensions::new();

            // Swap the session cookie: drop any incoming session (the
            // visitor's own identity — the share URL is a distinct context)
            // and inject the share identity's session id, so every handler
            // downstream sees a perfectly normal authenticated request.
            let cookie_name = crate::auth::session::SESSION_COOKIE;
            let mut kept: Vec<String> = Vec::new();
            if let Some(cookies) = parts
                .headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
            {
                for pair in cookies.split(';') {
                    let pair = pair.trim();
                    if pair.is_empty() {
                        continue;
                    }
                    let name = pair.split('=').next().unwrap_or("").trim();
                    if name != cookie_name {
                        kept.push(pair.to_string());
                    }
                }
            }
            kept.push(format!("{}={}", cookie_name, session_id));
            if let Ok(v) = axum::http::header::HeaderValue::from_str(&kept.join("; ")) {
                parts.headers.insert(axum::http::header::COOKIE, v);
            }

            inner.call(axum::http::Request::from_parts(parts, body)).await
        })
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

/// Rewrite libfw's public `/capabilities` advertisement to the app policy:
/// - range-based knobs keep `min`/`max` with `default` removed
/// - the adaptive ramp always starts at a conservative safe floor
/// - scalar knobs (`maxRetries`, `timeoutMs`) are emitted as a single config
///   value and never advertised as a range
#[derive(Clone)]
struct CapabilitiesRewrite<S> {
    inner: S,
    max_concurrency: u32,
    max_upload_window: u32,
    max_download_window: u32,
    max_chunk_size: u64,
    max_max_retries: u32,
    max_timeout_ms: u64,
    max_upload_size: u64,
    auto_tune: bool,
}

/// The chunk-size ceiling advertised to the adaptive engine.
///
/// libfw-client >= 0.4.4 does not ramp the chunk size: with `auto_tune` it
/// derives every request from the measured link (~100 ms of throughput) and
/// clamps the result into the advertised range, so the advertised `max` *is*
/// the ceiling a fast link may reach. That ceiling therefore comes from the
/// dedicated `auto_tune_max_chunk_size` (never below the fixed `chunk_size`,
/// which stays the floor). With `auto_tune` off the fixed `chunk_size` is
/// advertised as min, default and max alike.
fn chunk_size_ceiling(cfg: &crate::config::LibfwConfig) -> u64 {
    if cfg.auto_tune {
        cfg.auto_tune_max_chunk_size.max(cfg.chunk_size).max(1)
    } else {
        cfg.chunk_size.max(1)
    }
}

impl<S> CapabilitiesRewrite<S> {
    fn new(inner: S, cfg: &crate::config::LibfwConfig) -> Self {
        Self {
            inner,
            // Use configured ceilings directly; floors are applied when
            // rewriting depending on whether `auto_tune` is enabled.
            max_concurrency: cfg.concurrency.max(1),
            max_upload_window: cfg.upload_window.max(1),
            max_download_window: cfg.download_window.max(1),
            max_chunk_size: chunk_size_ceiling(cfg),
            max_max_retries: cfg.max_retries,
            max_timeout_ms: cfg.timeout_ms,
            max_upload_size: cfg.max_upload_size.max(1),
            auto_tune: cfg.auto_tune,
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
        let max_concurrency = self.max_concurrency;
        let max_upload_window = self.max_upload_window;
        let max_download_window = self.max_download_window;
        let max_chunk_size = self.max_chunk_size;
        let max_max_retries = self.max_max_retries;
        let max_timeout_ms = self.max_timeout_ms;
        let max_upload_size = self.max_upload_size;
        let auto_tune = self.auto_tune;
        let fut = self.inner.call(req);
        Box::pin(async move {
            let resp = fut.await.expect("inner service is infallible");
            if is_caps && resp.status() == StatusCode::OK {
                return Ok(rewrite_capabilities(
                    resp,
                    max_concurrency,
                    max_upload_window,
                    max_download_window,
                    max_chunk_size,
                    max_max_retries,
                    max_timeout_ms,
                    max_upload_size,
                    auto_tune,
                )
                .await);
            }
            Ok(resp)
        })
    }
}

/// Parse `/capabilities` JSON and rewrite each tuning knob to the app policy:
/// - range-based knobs keep `min`/`max` and omit `default`
/// - scalar knobs (`maxRetries`, `timeoutMs`) are emitted as a plain config
///   value, not a range
/// - both chunk-size knobs share one config value and the safe floor is 64 KiB
async fn rewrite_capabilities(
    resp: Response<Body>,
    max_concurrency: u32,
    max_upload_window: u32,
    max_download_window: u32,
    max_chunk_size: u64,
    max_max_retries: u32,
    max_timeout_ms: u64,
    max_upload_size: u64,
    auto_tune: bool,
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

    range_knob(limits, "concurrency", max_concurrency as i64, 1, auto_tune);
    range_knob(limits, "uploadWindow", max_upload_window as i64, 1, auto_tune);
    range_knob(limits, "downloadWindow", max_download_window as i64, 1, auto_tune);
    // Merge the upload/download chunk-size knobs into a single `chunkSize`
    // entry (libfw-client >= 0.4.4 has one shared knob).
    range_knob(limits, "chunkSize", max_chunk_size as i64, 64 * 1024, auto_tune);
    // Remove any separate downloadChunkSize entry so the frontend uses the
    // unified `chunkSize` config.
    if let Some(obj) = limits.as_object_mut() {
        obj.remove("downloadChunkSize");
    }
    scalar_knob(limits, "maxRetries", max_max_retries as i64);
    scalar_knob(limits, "timeoutMs", max_timeout_ms as i64);
    if let Some(obj) = limits.as_object_mut() {
        obj.insert("maxUploadSize".to_string(), serde_json::json!(max_upload_size as i64));
    }

    let rewritten = serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rewritten))
        .unwrap_or_else(|_| passthrough(bytes.to_vec()))
}

/// Rewrite a range-based capability knob to the app policy:
/// `min`/`default` are a low conservative start for the adaptive ramp, while
/// `max` follows the configured ceiling. With autotune off the browser sees a
/// fixed value; with autotune on it starts low and ramps upward until it hits
/// the operator's own ceiling.
fn range_knob(
    limits: &mut serde_json::Value,
    key: &str,
    configured_max: i64,
    ramp_floor: i64,
    auto_tune: bool,
) {
    let Some(item) = limits.as_object_mut().and_then(|o| o.get_mut(key)) else {
        return;
    };
    let Some(o) = item.as_object_mut() else {
        return;
    };
    // Compute the advertised start/default value: when autotune is enabled
    // the adaptive ramp should start at the conservative floor; when
    // autotune is disabled the server advertises a fixed value.
    let start_value = if auto_tune { ramp_floor } else { configured_max };

    // Preserve (or set) `default` so frontends that consume it continue
    // to work without changes; set it to the effective starting value.
    o.insert("default".to_string(), serde_json::json!(start_value));

    if auto_tune {
        // When autotune is enabled, advertise the conservative floor as
        // `min` and the configured ceiling as `max` (clamped to at least
        // the floor to avoid inverted ranges).
        let safe_max = configured_max.max(ramp_floor);
        o.insert("min".to_string(), serde_json::json!(ramp_floor));
        o.insert("max".to_string(), serde_json::json!(safe_max));
    } else {
        // When autotune is disabled, advertise a fixed value: min==max==cfg.
        o.insert("min".to_string(), serde_json::json!(configured_max));
        o.insert("max".to_string(), serde_json::json!(configured_max));
    }
}

/// Scalar knobs do not participate in a range; they always reflect the
/// configured value exactly.
fn scalar_knob(limits: &mut serde_json::Value, key: &str, value: i64) {
    let Some(obj) = limits.as_object_mut() else {
        return;
    };
    // Emit scalar knobs as plain numbers (upstream libfw format).
    obj.insert(key.to_string(), serde_json::json!(value));
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
    // A misconfigured zrip level policy is only a client-side hint, so it must
    // not be fatal: warn and let `config.js` fall back to "balanced" (the same
    // fallback libfw-client applies to an unknown `compressLevel`).
    if !config.libfw.compress_level_is_valid() {
        tracing::warn!(
            "[libfw] compress_level {:?} is not one of auto|fast|balanced|max|<number>; \
             falling back to \"balanced\"",
            config.libfw.compress_level
        );
    }
    // The level only steers the SDK while the server actually serves zrip:
    // with compression off the SDK's `compress` master switch sends identity
    // bodies regardless of the level policy.
    if config.libfw.compression_format() != libfw_core::compress::CompressionFormat::Zrip
        && !config.libfw.compress_level.trim().eq_ignore_ascii_case("balanced")
    {
        tracing::warn!(
            "[libfw] compress_level = {:?} has no effect while compression = {:?}",
            config.libfw.compress_level,
            config.libfw.compression
        );
    }
    // `max_fallback_bytes = 0` means "no limit" to libfw-client, which would
    // disable the only guard against an out-of-memory fallback download on
    // browsers without the File System Access API. The default is served
    // instead — say so loudly rather than silently ignoring the setting.
    if config.libfw.max_fallback_bytes == 0 {
        tracing::warn!(
            "[libfw] max_fallback_bytes = 0 disables the browser download-memory \
             limit; using the default of {} bytes instead",
            config.libfw.effective_max_fallback_bytes()
        );
    }

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
                match db.delete_expired_shares() {
                    Ok(n) => {
                        if n > 0 {
                            tracing::info!("Expired share link cleanup removed {n} rows");
                        }
                    }
                    Err(e) => tracing::warn!("Expired share link cleanup failed: {e}"),
                }
            }
        });
    }

    // libfw ships a built-in stale session-temp sweeper
    // (`spawn_stale_session_cleanup`): the concurrent upload protocol leaves a
    // `.libfw-sess-*` temp (plus a `.blocks` sidecar and its `.blocks.tmp`
    // rename sibling) behind whenever a browser dies mid-upload, and the
    // sweeper removes ones whose last write is older than the TTL — never
    // committed user files. Defaults: 1h sweep interval, 24h TTL.
    // Since 0.4.4 a client that disconnects mid-chunk KEEPS its temp + sidecar
    // (so a page refresh resumes instead of restarting), which is exactly what
    // this sweeper is for; `DirListingFilter` keeps them out of listings.
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

    let share_db = state.db.clone();

    let core = Router::new()
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
        // Recursive size of a path, for the download pre-flight on browsers
        // without the File System Access API (see `api::files::size`).
        .route("/api/files/size", get(api::files::size))
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
        // Temporary share links（分享）: authenticated management…
        .route("/api/files/share", post(api::share::create_share))
        .route("/api/files/shares", get(api::share::list_shares))
        .route("/api/files/share/{token}", delete(api::share::revoke_share))
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
        .with_state(state.clone());

    // Share routes（分享）: a share URL IS a temporary session. The wildcard
    // catch-all is fronted by ShareProxy, which swaps the URL token for the
    // identity's standard session cookie and rewrites the request onto the
    // normal routes above — so a share visitor is just a temporary, read-only
    // user flowing through the unmodified auth → ACL → libfw pipeline, and
    // every request they make stays under the share URL prefix.
    let share_proxy = ShareProxy::new(core.clone(), share_db);
    let mut app = Router::new()
        .route("/s/{token}", get(api::share::share_index))
        .route("/s/{token}/", get(api::share::share_page))
        .route("/s/{token}/{*path}", any_service(share_proxy))
        .with_state(state)
        .merge(core);

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
                "downloadChunkSize":  {"min": 65536, "max": 4194304, "default": 262144},
                "maxRetries":         {"min": 1, "max": 10, "default": 3},
                "timeoutMs":          {"min": 30000, "max": 1800000, "default": 600000},
                "maxUploadSize":      107374182400
            }
        }"#;
        Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap()
    }

    #[test]
    fn compress_level_json_maps_policies_and_numbers() {
        let cfg = |raw: &str| crate::config::LibfwConfig {
            compress_level: raw.to_string(),
            ..crate::config::LibfwConfig::default()
        };
        // Named policies are normalised to lowercase for the SDK.
        assert_eq!(cfg("max").compress_level_json(), serde_json::json!("max"));
        assert_eq!(cfg(" FAST ").compress_level_json(), serde_json::json!("fast"));
        assert_eq!(cfg("auto").compress_level_json(), serde_json::json!("auto"));
        // A bare number is an explicit zrip level.
        assert_eq!(cfg("3").compress_level_json(), serde_json::json!(3));
        assert_eq!(cfg("-8").compress_level_json(), serde_json::json!(-8));
        // Junk falls back to the SDK's own default and is flagged invalid.
        assert_eq!(
            cfg("turbo").compress_level_json(),
            serde_json::json!("balanced")
        );
        assert!(!cfg("turbo").compress_level_is_valid());
        assert!(cfg("auto").compress_level_is_valid());
        assert!(cfg("-8").compress_level_is_valid());
    }

    /// With adaptive tuning the advertised chunk-size ceiling is the dedicated
    /// knob (never below the fixed `chunk_size`); without it the fixed value is
    /// advertised as min, default and max alike.
    #[test]
    fn chunk_size_ceiling_follows_auto_tune() {
        let fixed = crate::config::LibfwConfig {
            chunk_size: 2 * 1024 * 1024,
            ..crate::config::LibfwConfig::default()
        };
        assert_eq!(chunk_size_ceiling(&fixed), 2 * 1024 * 1024);

        let tuned = crate::config::LibfwConfig {
            auto_tune: true,
            chunk_size: 2 * 1024 * 1024,
            auto_tune_max_chunk_size: 8 * 1024 * 1024,
            ..crate::config::LibfwConfig::default()
        };
        assert_eq!(chunk_size_ceiling(&tuned), 8 * 1024 * 1024);

        // A ceiling below the fixed chunk size must never shrink the ceiling
        // the ramp starts from.
        let small = crate::config::LibfwConfig {
            auto_tune: true,
            chunk_size: 4 * 1024 * 1024,
            auto_tune_max_chunk_size: 1024 * 1024,
            ..crate::config::LibfwConfig::default()
        };
        assert_eq!(chunk_size_ceiling(&small), 4 * 1024 * 1024);

        // `auto_tune_max_chunk_size` is inert while tuning is off.
        let off = crate::config::LibfwConfig {
            auto_tune: false,
            chunk_size: 1024 * 1024,
            auto_tune_max_chunk_size: 16 * 1024 * 1024,
            ..crate::config::LibfwConfig::default()
        };
        assert_eq!(chunk_size_ceiling(&off), 1024 * 1024);
    }

    /// The rewrite keeps a fixed safe floor for the ramping engine, removes the
    /// default field so the SDK starts from `min`, and lets `max` follow the
    /// configured value. This keeps the advertised range valid while allowing
    /// libfw to adapt upward from a conservative starting point.
    #[tokio::test]
    async fn capabilities_rewrite_uses_min_start_and_configured_max() {
        let resp = rewrite_capabilities(
            caps_response(),
            4,                 // concurrency
            4,                 // uploadWindow
            4,                 // downloadWindow
            8 * 1024 * 1024,   // chunkSize = 8 MiB
            7,                 // maxRetries
            123_456,           // timeoutMs
            64 * 1024 * 1024,  // maxUploadSize
            true,
        )
        .await;

        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let limits = &v["limits"];

        for key in ["concurrency", "uploadWindow", "downloadWindow", "chunkSize"] {
            let mn = limits[key]["min"].as_i64().unwrap_or(i64::MAX);
            let mx = limits[key]["max"].as_i64().unwrap_or(1);
            assert!(
                mn <= mx,
                "limits.{key} inverted: min {mn} > max {mx} after rewrite"
            );
            // default must be present and equal to the advertised start value
            let def = limits[key].get("default").and_then(|v| v.as_i64());
            assert!(def.is_some(), "{key}.default must be present");
        }

        assert_eq!(limits["concurrency"]["min"].as_i64(), Some(1));
        assert_eq!(limits["concurrency"]["max"].as_i64(), Some(4));
        assert_eq!(limits["uploadWindow"]["min"].as_i64(), Some(1));
        assert_eq!(limits["uploadWindow"]["max"].as_i64(), Some(4));
        assert_eq!(limits["downloadWindow"]["min"].as_i64(), Some(1));
        assert_eq!(limits["downloadWindow"]["max"].as_i64(), Some(4));
        assert_eq!(limits["chunkSize"]["min"].as_i64(), Some(64 * 1024));
        assert_eq!(limits["chunkSize"]["max"].as_i64(), Some(8 * 1024 * 1024));
        // downloadChunkSize is removed in favour of a unified chunkSize
        assert_eq!(limits["maxRetries"].as_i64(), Some(7));
        assert_eq!(limits["timeoutMs"].as_i64(), Some(123_456));

        assert_eq!(v["limits"]["maxUploadSize"].as_i64(), Some(64 * 1024 * 1024));
    }

    /// If the configured max is below the safe floor, the range is still kept
    /// valid by clamping the max back to the fixed floor. The SDK then ramps
    /// from `min` without ever producing an inverted capability range.
    #[tokio::test]
    async fn capabilities_rewrite_keeps_valid_range_when_config_is_too_small() {
        let resp = rewrite_capabilities(caps_response(), 1, 1, 1, 1024, 2, 2048, 4096, true).await;
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let limits = &v["limits"];
        for key in ["concurrency", "uploadWindow", "downloadWindow", "chunkSize"] {
            let mn = limits[key]["min"].as_i64().unwrap_or(i64::MAX);
            let mx = limits[key]["max"].as_i64().unwrap_or(1);
            assert!(mn <= mx, "limits.{key} inverted after pin");
            let def = limits[key].get("default").and_then(|v| v.as_i64());
            assert!(def.is_some(), "{key}.default must be present");
        }

        assert_eq!(limits["chunkSize"]["min"].as_i64(), Some(64 * 1024));
        assert_eq!(limits["chunkSize"]["max"].as_i64(), Some(64 * 1024));
        assert_eq!(limits["maxRetries"].as_i64(), Some(2));
        assert_eq!(limits["timeoutMs"].as_i64(), Some(2048));
        assert_eq!(v["limits"]["maxUploadSize"].as_i64(), Some(4096));
    }
}
