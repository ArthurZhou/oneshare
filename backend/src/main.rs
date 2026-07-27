mod acl;
mod api;
mod auth;
mod config;
mod db;
mod models;

use crate::auth::oidc::OidcClient;
use crate::auth::session::SessionManager;
use crate::config::Config;
use crate::db::Database;
use axum::{
    routing::{delete, get, post, put},
    Router,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

pub struct AppState {
    pub db: Database,
    pub config: Config,
    pub oidc_client: OidcClient,
    pub oidc_states: Mutex<HashMap<String, ()>>,
    pub session_manager: SessionManager,
    pub wfw_handle: wfw_server::WfwHandle,
    pub hmac_key: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oneshare_backend=debug,wfw_server=info".into()),
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

    let uploads_dir = config.root_dir().join("uploads");
    std::fs::create_dir_all(&uploads_dir).expect("Failed to create uploads dir");

    let (wfw_router, wfw_handle) = wfw_server::init(wfw_server::WfwConfig {
        root: config.root_dir().clone(),
        allowed_upload_prefixes: vec![PathBuf::from("uploads")],
        allowed_download_prefixes: vec![PathBuf::from("")],
        hmac_secret: Some(hmac_secret.clone()),
        ..Default::default()
    });

    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        oidc_client,
        oidc_states: Mutex::new(HashMap::new()),
        session_manager,
        wfw_handle,
        hmac_key: Some(hmac_secret.clone()),
    });

    // Embed wfw into the main axum server by converting it to a resolved service
    let wfw_service = wfw_router.into_service();

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
        .route("/api/files/wfw-token", get(api::files::get_wfw_token))
        .route("/api/admin/users", get(api::admin::list_users))
        .route("/api/admin/groups", get(api::admin::list_groups))
        .route("/api/admin/groups", post(api::admin::create_group))
        .route("/api/admin/groups/{id}", delete(api::admin::delete_group))
        .route("/api/admin/groups/add-user", post(api::admin::add_user_to_group))
        .route("/api/admin/groups/remove-user", post(api::admin::remove_user_from_group))
        .route("/api/admin/acl", get(api::admin::list_acl))
        .route("/api/admin/acl", post(api::admin::set_acl))
        .route("/api/admin/acl/{id}", delete(api::admin::remove_acl))
        .nest_service("/wfw", wfw_service)
        .nest_service("/", ServeDir::new("../frontend"))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", config.listen_addr(), config.listen_port());
    tracing::info!("OneShare starting on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
