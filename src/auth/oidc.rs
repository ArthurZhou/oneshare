use crate::config::OidcConfig;
use reqwest::Client;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use tracing;

/// OIDC Discovery document (subset of fields we need).
/// Per RFC 8414 / OpenID Connect Discovery 1.0.
#[derive(Debug, Clone, Deserialize)]
struct OidcDiscovery {
    issuer: Option<String>,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    userinfo_endpoint: Option<String>,
}

#[derive(Clone)]
pub struct OidcClient {
    client: Client,
    issuer_url: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
}

impl OidcClient {
    pub async fn new(config: &OidcConfig) -> Result<Self, String> {
        let base = config.issuer_url.trim_end_matches('/');

        // reqwest is compiled with `rustls-no-provider` and we supply `ring`
        // as the crypto provider (instead of the default `aws-lc-rs`, so the
        // build needs no cmake). rustls requires the provider to be installed
        // explicitly before any client is built. Ignore the error if a
        // provider is already installed (e.g. on a second OidcClient).
        let _ = rustls::crypto::ring::default_provider().install_default();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        // ---- Step 1: try OIDC Discovery via .well-known ------------------
        // If ALL three endpoints are explicitly configured, skip discovery.
        // Otherwise, fetch the discovery document from the issuer.
        let all_explicit = config.authorization_endpoint.is_some()
            && config.token_endpoint.is_some()
            && config.userinfo_endpoint.is_some();

        let discovery = if all_explicit {
            tracing::info!(
                "All OIDC endpoints explicitly configured, skipping well-known discovery."
            );
            None
        } else {
            Self::fetch_discovery(&client, base).await
        };

        // ---- Step 2: resolve endpoints (explicit config > discovery > error)
        let authorization_endpoint = config
            .authorization_endpoint
            .clone()
            .or_else(|| {
                discovery
                    .as_ref()
                    .and_then(|d| d.authorization_endpoint.clone())
            })
            .ok_or_else(|| {
                format!(
                    "OIDC authorization_endpoint not found. \
                     Configure it explicitly or ensure the issuer supports \
                     discovery at {}/.well-known/openid-configuration",
                    base
                )
            })?;

        let token_endpoint = config
            .token_endpoint
            .clone()
            .or_else(|| {
                discovery
                    .as_ref()
                    .and_then(|d| d.token_endpoint.clone())
            })
            .ok_or_else(|| {
                format!(
                    "OIDC token_endpoint not found. \
                     Configure it explicitly or ensure the issuer supports \
                     discovery at {}/.well-known/openid-configuration",
                    base
                )
            })?;

        let userinfo_endpoint = config
            .userinfo_endpoint
            .clone()
            .or_else(|| {
                discovery
                    .as_ref()
                    .and_then(|d| d.userinfo_endpoint.clone())
            })
            .ok_or_else(|| {
                format!(
                    "OIDC userinfo_endpoint not found. \
                     Configure it explicitly or ensure the issuer supports \
                     discovery at {}/.well-known/openid-configuration",
                    base
                )
            })?;

        tracing::info!(
            "OIDC client initialized. issuer={}, authorization={}, token={}, userinfo={}",
            base,
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
        );

        Ok(OidcClient {
            client,
            issuer_url: config.issuer_url.clone(),
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            redirect_uri: config.redirect_uri.clone(),
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
        })
    }

    /// Fetch the OIDC Discovery document from the issuer.
    /// Returns None on any failure (network error, non-200, bad JSON, missing fields).
    async fn fetch_discovery(client: &Client, issuer: &str) -> Option<OidcDiscovery> {
        let discovery_url = format!("{}/.well-known/openid-configuration", issuer);

        tracing::debug!("Fetching OIDC discovery document: {}", discovery_url);

        let resp = match client.get(&discovery_url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "OIDC discovery request failed for {}: {}. \
                     Falling back to explicit endpoint configuration.",
                    discovery_url, e
                );
                return None;
            }
        };

        let status = resp.status();
        if !status.is_success() {
            tracing::warn!(
                "OIDC discovery returned HTTP {} for {}. \
                 Falling back to explicit endpoint configuration.",
                status.as_u16(),
                discovery_url,
            );
            return None;
        }

        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "OIDC discovery: failed to read response body: {}. \
                     Falling back to explicit endpoint configuration.",
                    e
                );
                return None;
            }
        };

        let discovery: OidcDiscovery = match serde_json::from_str(&body) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    "OIDC discovery: failed to parse JSON from {}: {}. \
                     Falling back to explicit endpoint configuration.",
                    discovery_url, e
                );
                return None;
            }
        };

        // Validate that the discovered issuer matches (if present)
        if let Some(ref discovered_issuer) = discovery.issuer {
            if discovered_issuer != issuer {
                tracing::warn!(
                    "OIDC discovery issuer mismatch: configured={}, discovered={}. \
                     Proceeding with discovered endpoints (ID token is not validated).",
                    issuer, discovered_issuer,
                );
            }
        }

        let has_all = discovery.authorization_endpoint.is_some()
            && discovery.token_endpoint.is_some()
            && discovery.userinfo_endpoint.is_some();

        if !has_all {
            tracing::warn!(
                "OIDC discovery document from {} is missing some required endpoints. \
                 auth={:?}, token={:?}, userinfo={:?}. \
                 Falling back to explicit endpoint configuration.",
                discovery_url,
                discovery.authorization_endpoint,
                discovery.token_endpoint,
                discovery.userinfo_endpoint,
            );
            return None;
        }

        tracing::debug!(
            "OIDC discovery succeeded: auth={}, token={}, userinfo={}",
            discovery.authorization_endpoint.as_deref().unwrap_or("?"),
            discovery.token_endpoint.as_deref().unwrap_or("?"),
            discovery.userinfo_endpoint.as_deref().unwrap_or("?"),
        );

        Some(discovery)
    }

    /// Build the authorization URL. Returns (url, csrf_state).
    pub fn authorize_url(&self) -> (String, String) {
        let csrf_state = uuid::Uuid::new_v4().to_string();

        let url = format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&state={}&scope=openid+profile+email",
            self.authorization_endpoint,
            self.client_id,
            self.redirect_uri,
            csrf_state,
        );

        tracing::debug!(
            "Generated authorize URL: {}... (csrf_state={})",
            &url[..url.len().min(200)],
            csrf_state,
        );

        (url, csrf_state)
    }

    /// Exchange an authorization code for user info.
    /// Does NOT validate any ID token — just gets userinfo via access_token,
    /// matching the approach in example.py.
    pub async fn exchange_code(&self, code: &str) -> Result<OidcUserInfo, String> {
        tracing::debug!(
            "Exchanging authorization code: {}...",
            &code.chars().take(20).collect::<String>(),
        );

        // Step 1: POST to token endpoint (application/x-www-form-urlencoded)
        let form_body = {
            let mut s = url::form_urlencoded::Serializer::new(String::new());
            s.append_pair("grant_type", "authorization_code");
            s.append_pair("code", code);
            s.append_pair("client_id", &self.client_id);
            s.append_pair("client_secret", &self.client_secret);
            s.append_pair("redirect_uri", &self.redirect_uri);
            s.finish()
        };

        let token_resp = self
            .client
            .post(&self.token_endpoint)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("Token endpoint request failed: {}", e);
                format!("Token request failed: {}", e)
            })?;

        let token_status = token_resp.status();
        let token_body = token_resp
            .text()
            .await
            .map_err(|e| format!("Failed to read token response: {}", e))?;

        if !token_status.is_success() {
            return Err(format!(
                "Token endpoint returned HTTP {}: {}",
                token_status, token_body
            ));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: Option<String>,
        }

        let token_data: TokenResponse =
            serde_json::from_str(&token_body).map_err(|e| {
                tracing::error!(
                    "Failed to parse token response JSON: {}. Body: {}",
                    e,
                    token_body,
                );
                format!("Failed to parse token response: {}. Body: {}", e, token_body)
            })?;

        let access_token = token_data.access_token.ok_or_else(|| {
            let msg = format!("No access_token in token response: {}", token_body);
            tracing::error!("{}", msg);
            msg
        })?;

        tracing::debug!("Got access_token (length={})", access_token.len());

        // Step 2: GET userinfo with Bearer token
        let user_resp = self
            .client
            .get(&self.userinfo_endpoint)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
            .map_err(|e| {
                tracing::error!("Userinfo request failed: {}", e);
                format!("Userinfo request failed: {}", e)
            })?;

        let user_status = user_resp.status();
        let user_body = user_resp
            .text()
            .await
            .map_err(|e| format!("Failed to read userinfo response: {}", e))?;

        if !user_status.is_success() {
            return Err(format!(
                "Userinfo endpoint returned HTTP {}: {}",
                user_status, user_body
            ));
        }

        #[derive(Deserialize)]
        struct UserInfoResponse {
            sub: Option<String>,
            name: Option<String>,
            email: Option<String>,
            preferred_username: Option<String>,
        }

        let user_data: UserInfoResponse =
            serde_json::from_str(&user_body).map_err(|e| {
                tracing::error!(
                    "Failed to parse userinfo response JSON: {}. Body: {}",
                    e,
                    user_body,
                );
                format!(
                    "Failed to parse userinfo response: {}. Body: {}",
                    e, user_body
                )
            })?;

        let sub = user_data.sub.unwrap_or_default();
        let name = user_data
            .name
            .or(user_data.preferred_username)
            .unwrap_or_else(|| sub.clone());
        let email = user_data.email.unwrap_or_default();

        tracing::debug!(
            "User info extracted: sub={}, name={}, email={}",
            sub, name, email,
        );

        if sub.is_empty() {
            return Err(format!(
                "Userinfo response missing 'sub' field. Body: {}",
                user_body
            ));
        }

        Ok(OidcUserInfo { sub, name, email })
    }
}

#[derive(Debug, Clone)]
pub struct OidcUserInfo {
    pub sub: String,
    pub name: String,
    pub email: String,
}
