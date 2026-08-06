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
    routing::{any_service, delete, get, post, put},
    Router,
};
use libfw_core::auth::PathValidator;
use libfw_core::compress::CompressionFormat;
use libfw_server::{router as libfw_router, FsStorage, ServerState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tower::Service;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

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
        .route("/api/admin/groups/add-user", post(api::admin::add_user_to_group))
        .route("/api/admin/groups/remove-user", post(api::admin::remove_user_from_group))
        .route("/api/admin/acl", get(api::admin::list_acl))
        .route("/api/admin/acl", post(api::admin::set_acl))
        .route("/api/admin/acl/{id}", delete(api::admin::remove_acl))
        // libfw file transfer routes (uses its own state, embedded via any_service).
        // FreshPathParams clears the outer router's `{*path}` captures so libfw's
        // own path match is the only one its `Path` extractor sees.
        .route("/file/{*path}", any_service(FreshPathParams::new(libfw_app.clone())))
        .route("/dir/{*path}", any_service(FreshPathParams::new(libfw_app)))
        .fallback_service(ServeDir::new("./frontend"))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", config.listen_addr(), config.listen_port());
    tracing::info!("OneShare starting on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
