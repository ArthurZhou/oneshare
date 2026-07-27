use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub oidc: OidcConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub listen_port: u16,
    pub root_dir: PathBuf,
    pub database_url: String,
    pub session_cookie_key: String,
    pub hmac_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    /// Optional: authorization endpoint override.
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub authorization_endpoint: Option<String>,
    /// Optional: token endpoint override.
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub token_endpoint: Option<String>,
    /// Optional: userinfo endpoint override.
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub userinfo_endpoint: Option<String>,
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file '{}': {}", path, e))?;
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse config file '{}': {}", path, e))
    }

    pub fn listen_addr(&self) -> &str {
        &self.server.listen_addr
    }

    pub fn listen_port(&self) -> u16 {
        self.server.listen_port
    }

    pub fn root_dir(&self) -> &PathBuf {
        &self.server.root_dir
    }

    pub fn database_url(&self) -> &str {
        &self.server.database_url
    }

    pub fn session_cookie_key(&self) -> &str {
        &self.server.session_cookie_key
    }

    pub fn hmac_secret(&self) -> &str {
        &self.server.hmac_secret
    }

    pub fn wfw_root(&self) -> PathBuf {
        self.server.root_dir.join("wfw")
    }
}
