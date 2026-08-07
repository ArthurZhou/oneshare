mod acl;
mod api;
mod auth;
mod config;
mod db;
mod libtoken;
mod models;

use crate::auth::oidc::OidcClient;
use crate::auth::session::SessionManager;
use crate::config::Config;
use crate::db::Database;
use crate::libtoken::OneshareTokenVerifier;
use axum::{
    body::Body,
    http::{Request, Response, StatusCode, Uri},
    routing::{any_service, delete, get, post, put},
    Router,
};
use libfw_core::auth::PathValidator;
use libfw_core::compress::CompressionFormat;
use libfw_server::{router as libfw_router, FsStorage, ServerState};
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tower::{Layer, Service, ServiceBuilder};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

pub struct AppState {
    pub db: Database,
    pub config: Config,
    pub oidc_client: OidcClient,
    pub oidc_states: Mutex<HashMap<String, ()>>,
    pub session_manager: SessionManager,
    pub hmac_key: String,
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

    std::fs::create_dir_all(config.root_dir()).expect("Failed to create root dir");

    let db = Database::new(config.database_url()).expect("Failed to initialize database");

    let oidc_client = crate::auth::oidc::OidcClient::new(&config.oidc)
        .await
        .expect("Failed to initialize OIDC client");

    let session_manager = SessionManager::new();

    let hmac_secret = config.hmac_secret().to_string();

    // ── libfw file-transfer layer ──
    let libfw_state = Arc::new(
        ServerState::builder()
            .storage(FsStorage::new(config.root_dir()))
            .verifier(OneshareTokenVerifier {
                hmac_key: hmac_secret.clone(),
            })
            .validator(PathValidator::new())
            // The web frontend fetches `/file/*` and saves the raw body, so
            // compression must be disabled: browsers advertise
            // `Accept-Encoding: zstd`, which libfw maps to its proprietary
            // zrip framing; since no `Content-Encoding` header is sent, the
            // browser would save the compressed bytes as the file itself.
            .compression(CompressionFormat::None)
            .build(),
    );

    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        oidc_client,
        oidc_states: Mutex::new(HashMap::new()),
        session_manager,
        hmac_key: hmac_secret,
    });

    let libfw_app = libfw_router(libfw_state);

    let app = Router::new()
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
        // libfw file transfer routes (uses its own state, embedded via any_service).
        // FreshPathParams clears the outer router's `{*path}` captures so libfw's
        // own path match is the only one its `Path` extractor sees.
        .route("/file/{*path}", any_service(FreshPathParams::new(libfw_app.clone())))
        .route("/dir/{*path}", any_service(FreshPathParams::new(libfw_app)))
        // Serve the frontend with revalidation: without a Cache-Control header,
        // browsers heuristically cache JS/CSS, which caused stale `app.js` to be
        // mixed with a newer `file-explorer.js` (and a spurious upload error).
        .fallback_service(
            SetResponseHeaderLayer::overriding(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-cache, must-revalidate"),
            )
            .layer(ServeDir::new("./frontend")),
        )
        .layer(CorsLayer::permissive())
        .with_state(state);

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
