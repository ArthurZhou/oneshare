use crate::config::OidcConfig;
use base64::Engine;
use reqwest::Client;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tracing;

/// OIDC Discovery document (subset of fields we need).
/// Per RFC 8414 / OpenID Connect Discovery 1.0.
#[derive(Debug, Clone, Deserialize)]
struct OidcDiscovery {
    issuer: Option<String>,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    userinfo_endpoint: Option<String>,
    jwks_uri: Option<String>,
}

/// A JSON Web Key Set (RFC 7517) — the provider's public keys used to
/// verify ID token signatures.
#[derive(Debug, Clone, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kty: Option<String>,
    kid: Option<String>,
    /// Parsed for completeness; selection is by `kid`/`kty` (alg must match
    /// RS256, which is enforced on the token header before key lookup).
    #[allow(dead_code)]
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

/// Unverified JOSE header of an ID token (only `alg`/`kid` are needed to
/// pick the right verification key).
#[derive(Debug, Clone, Deserialize)]
struct IdTokenHeader {
    alg: Option<String>,
    kid: Option<String>,
}

/// Claims extracted from a (verified) ID token.
#[derive(Debug, Clone, Deserialize)]
struct IdTokenClaims {
    iss: Option<String>,
    #[serde(default)]
    aud: Option<serde_json::Value>,
    sub: Option<String>,
    exp: Option<i64>,
    nonce: Option<String>,
}

#[derive(Clone)]
pub struct OidcClient {
    client: Client,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    /// The issuer we trust. ID tokens whose `iss` differs are rejected.
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
    jwks_uri: String,
    /// Cached JWKS (loaded lazily on first token exchange, refreshed on
    /// verification failure). `Arc` keeps the client `Clone` (Mutex is not).
    jwks: Arc<Mutex<Option<JwkSet>>>,
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
        // If ALL endpoints are explicitly configured (plus jwks_uri), skip
        // discovery. Otherwise, fetch the discovery document from the issuer.
        let all_explicit = config.authorization_endpoint.is_some()
            && config.token_endpoint.is_some()
            && config.userinfo_endpoint.is_some()
            && config.jwks_uri.is_some();

        let discovery = if all_explicit {
            tracing::info!(
                "All OIDC endpoints explicitly configured, skipping well-known discovery."
            );
            None
        } else {
            Self::fetch_discovery(&client, base).await
        };

        // The discovered issuer MUST match the configured issuer. Trusting
        // endpoints that claim a different issuer would let a MITM'd or
        // hijacked discovery document substitute its own token/userinfo
        // endpoints and mint identities. Fail startup instead of warning.
        if let Some(ref discovered_issuer) = discovery.as_ref().and_then(|d| d.issuer.clone()) {
            if discovered_issuer.trim_end_matches('/') != base {
                return Err(format!(
                    "OIDC discovery issuer mismatch: configured={}, discovered={}. \
                     Refusing to start: the provider's discovery document does not \
                     identify itself as the configured issuer_url.",
                    base, discovered_issuer,
                ));
            }
        }

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

        let jwks_uri = config
            .jwks_uri
            .clone()
            .or_else(|| {
                discovery
                    .as_ref()
                    .and_then(|d| d.jwks_uri.clone())
            })
            .ok_or_else(|| {
                format!(
                    "OIDC jwks_uri not found. ID token signature verification \
                     requires the provider's JSON Web Key Set. Configure it \
                     explicitly or ensure the issuer supports discovery at \
                     {}/.well-known/openid-configuration",
                    base
                )
            })?;

        tracing::info!(
            "OIDC client initialized. issuer={}, authorization={}, token={}, userinfo={}, jwks={}",
            base,
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
            jwks_uri,
        );

        Ok(OidcClient {
            client,
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            redirect_uri: config.redirect_uri.clone(),
            issuer: base.to_string(),
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
            jwks_uri,
            jwks: Arc::new(Mutex::new(None)),
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

    /// Build the authorization URL. Returns (url, csrf_state, nonce).
    ///
    /// The `state` guards against login-CSRF; the `nonce` is echoed back by
    /// the provider and validated on the callback for replay/tamper
    /// protection. Query parameters are percent-encoded so a redirect_uri or
    /// client_id with special characters survives the round-trip.
    pub fn authorize_url(&self) -> (String, String, String) {
        let csrf_state = uuid::Uuid::new_v4().to_string();
        let nonce = uuid::Uuid::new_v4().to_string();

        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("state", &csrf_state)
            .append_pair("nonce", &nonce)
            .append_pair("scope", "openid profile email")
            .finish();

        let url = format!("{}?{}", self.authorization_endpoint, query);

        tracing::debug!(
            "Generated authorize URL: {}... (csrf_state={}, nonce={})",
            &url[..url.len().min(200)],
            csrf_state,
            nonce,
        );

        (url, csrf_state, nonce)
    }

    /// Exchange an authorization code for user info.
    ///
    /// The ID token in the token response is **verified** before anything
    /// else is trusted: signature (RS256 against the provider's JWKS),
    /// `iss` (must equal the configured issuer), `aud` (must contain our
    /// client_id), `exp`, and `nonce` (must equal the nonce we sent with
    /// the authorization request — replay/tamper protection). Userinfo is
    /// fetched only after verification, and its `sub` must match the ID
    /// token's `sub`.
    pub async fn exchange_code(&self, code: &str, expected_nonce: &str) -> Result<OidcUserInfo, String> {
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
            id_token: Option<String>,
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

        // Step 1.5: verify the ID token. This is the linchpin of the whole
        // flow: without it, a compromised token endpoint (or a MITM'd
        // discovery document redirecting the token exchange) could mint any
        // identity. Refuse to proceed without a verifiable ID token.
        let id_token = token_data.id_token.ok_or_else(|| {
            let msg = format!(
                "No id_token in token response ({}). The provider must return a \
                 signed ID token for OpenID Connect flows.",
                self.token_endpoint
            );
            tracing::error!("{}", msg);
            msg
        })?;
        let claims = self
            .verify_id_token(&id_token, expected_nonce)
            .await
            .map_err(|e| {
                tracing::error!("ID token verification failed: {}", e);
                format!("ID token verification failed: {e}")
            })?;

        let id_sub = claims.sub.unwrap_or_default();

        // Step 2: GET userinfo with Bearer token
        let access_token = token_data.access_token.ok_or_else(|| {
            let msg = format!("No access_token in token response: {}", token_body);
            tracing::error!("{}", msg);
            msg
        })?;

        tracing::debug!("Got access_token (length={})", access_token.len());

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
            role: Option<String>,
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

        // The userinfo `sub` must identify the same end-user as the verified
        // ID token — otherwise a rogue userinfo endpoint could swap the
        // identity the session is minted for.
        if let Some(ui_sub) = &user_data.sub {
            if !ui_sub.is_empty() && ui_sub != &id_sub {
                return Err(format!(
                    "Userinfo sub mismatch: id_token sub={id_sub}, userinfo sub={ui_sub}"
                ));
            }
        }

        let name = user_data
            .name
            .or(user_data.preferred_username)
            .unwrap_or_else(|| id_sub.clone());
        let email = user_data.email.unwrap_or_default();
        // Optional `role` claim (may be a plain string or, e.g., a JSON
        // array). Multiple roles may be separated by a comma (`,` or the
        // full-width `，`), with or without surrounding spaces — e.g.
        // "ops,staff", "ops, staff" or "ops ，staff" all split into
        // ["ops", "staff"]. Each role is trimmed and matched against
        // groups sequentially.
        let roles: Vec<String> = user_data
            .role
            .map(|r| {
                r.split([',', '，'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        tracing::debug!(
            "User info extracted: sub={}, name={}, email={}, roles={:?}",
            id_sub, name, email, roles,
        );

        if id_sub.is_empty() {
            return Err("ID token missing 'sub' claim".to_string());
        }

        Ok(OidcUserInfo { sub: id_sub, name, email, roles })
    }

    /// Load the provider's JWKS, caching it. On verification failure the
    /// caller retries once with a forced refresh (covers key rotation).
    async fn load_jwks(&self, force: bool) -> Result<JwkSet, String> {
        if !force {
            if let Some(set) = self.jwks.lock().unwrap().as_ref() {
                return Ok(set.clone());
            }
        }
        let resp = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|e| format!("JWKS request failed: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("JWKS endpoint returned HTTP {}", status.as_u16()));
        }
        let set: JwkSet = resp.json().await.map_err(|e| {
            format!("Failed to parse JWKS from {}: {}", self.jwks_uri, e)
        })?;
        if set.keys.is_empty() {
            return Err("JWKS contains no keys".to_string());
        }
        *self.jwks.lock().unwrap() = Some(set.clone());
        Ok(set)
    }

    /// Verify an ID token per OIDC Core 3.1.3.7: RS256 signature against
    /// the provider JWKS, then `iss`/`aud`/`exp`/`nonce` claims.
    async fn verify_id_token(&self, id_token: &str, expected_nonce: &str) -> Result<IdTokenClaims, String> {
        let segments: Vec<&str> = id_token.split('.').collect();
        if segments.len() != 3 {
            return Err("ID token must have 3 dot-separated segments".to_string());
        }
        let signing_input = format!("{}.{}", segments[0], segments[1]);
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segments[2])
            .map_err(|e| format!("ID token signature base64 decode: {e}"))?;

        let header: IdTokenHeader = decode_jwt_segment(segments[0])?;
        if header.alg.as_deref() != Some("RS256") {
            return Err(format!(
                "ID token alg {:?} not supported (only RS256)",
                header.alg
            ));
        }

        // Try the cached JWKS, then one forced refresh (key rotation).
        let mut jwks = self.load_jwks(false).await?;
        let mut verified = verify_with_jwks(&jwks, &header, &signing_input, &signature);
        if verified.is_err() {
            tracing::warn!(
                "ID token verification with cached JWKS failed; refreshing keyset: {}",
                verified.as_ref().err().map(|e| e.as_str()).unwrap_or("")
            );
            jwks = self.load_jwks(true).await?;
            verified = verify_with_jwks(&jwks, &header, &signing_input, &signature);
        }
        verified?;

        let claims: IdTokenClaims = decode_jwt_segment(segments[1])?;

        // iss: must be exactly the configured issuer (trailing-slash tolerant).
        let iss_ok = claims
            .iss
            .as_deref()
            .map(|i| i.trim_end_matches('/') == self.issuer)
            .unwrap_or(false);
        if !iss_ok {
            return Err(format!(
                "ID token iss {:?} != configured issuer {}",
                claims.iss, self.issuer
            ));
        }

        // aud: must contain our client_id (string or array form).
        let aud_ok = match claims.aud.as_ref() {
            None => false,
            Some(serde_json::Value::String(s)) => s == &self.client_id,
            Some(serde_json::Value::Array(arr)) => {
                arr.iter().any(|v| v.as_str() == Some(self.client_id.as_str()))
            }
            _ => false,
        };
        if !aud_ok {
            return Err(format!(
                "ID token aud {:?} does not include client_id {}",
                claims.aud, self.client_id
            ));
        }

        // exp: allow 60s clock skew.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        match claims.exp {
            Some(exp) if exp >= now - 60 => {}
            Some(exp) => return Err(format!("ID token expired (exp={exp}, now={now})")),
            None => return Err("ID token missing exp claim".to_string()),
        }

        // nonce: must match what we sent in the authorization request. This
        // is the replay/tamper protection for the login; a missing or
        // mismatched nonce is rejected outright.
        match claims.nonce.as_deref() {
            Some(n) if n == expected_nonce => {}
            _ => {
                return Err("ID token nonce missing or does not match the login request".to_string());
            }
        }

        Ok(claims)
    }
}

/// Base64url-decode and JSON-parse one JWT segment (header or payload).
fn decode_jwt_segment<T: serde::de::DeserializeOwned>(seg: &str) -> Result<T, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg)
        .map_err(|e| format!("JWT segment base64 decode: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("JWT segment JSON parse: {e}"))
}

/// Verify an RS256 signature against the JWKS, selecting the key by `kid`
/// (or the only key when no `kid`/no match).
fn verify_with_jwks(
    jwks: &JwkSet,
    header: &IdTokenHeader,
    signing_input: &str,
    signature: &[u8],
) -> Result<(), String> {
    use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};

    let keys: Vec<&Jwk> = jwks
        .keys
        .iter()
        .filter(|k| k.kty.as_deref().unwrap_or("") == "RSA")
        .collect();
    if keys.is_empty() {
        return Err("JWKS has no RSA keys".to_string());
    }

    let key = match header.kid.as_deref() {
        Some(kid) => keys
            .iter()
            .find(|k| k.kid.as_deref() == Some(kid))
            .copied()
            .ok_or_else(|| format!("no JWK with kid={kid}"))?,
        None => {
            if keys.len() == 1 {
                keys[0]
            } else {
                return Err("ID token has no kid and JWKS has multiple keys".to_string());
            }
        }
    };

    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key.n.as_deref().ok_or("JWK missing n")?)
        .map_err(|e| format!("JWK n base64 decode: {e}"))?;
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key.e.as_deref().ok_or("JWK missing e")?)
        .map_err(|e| format!("JWK e base64 decode: {e}"))?;

    // ring 0.17's `RsaPublicKeyComponents` does not implement `AsRef<[u8]>`
    // (so it cannot feed `UnparsedPublicKey`); it exposes its own `verify`
    // taking an `RsaParameters`. `RSA_PKCS1_2048_8192_SHA256` is exactly the
    // RS256 verification parameters (PKCS#1 v1.5 padding + SHA-256, keys of
    // 2048–8192 bits) — the standard algorithm for OIDC providers.
    let public_key = RsaPublicKeyComponents { n, e };
    public_key
        .verify(&RSA_PKCS1_2048_8192_SHA256, signing_input.as_bytes(), signature)
        .map_err(|_| "ID token signature verification failed".to_string())
}

#[derive(Debug, Clone)]
pub struct OidcUserInfo {
    pub sub: String,
    pub name: String,
    pub email: String,
    /// Optional `role` claim returned by the provider's userinfo endpoint.
    /// Multiple roles may be present, separated by commas.
    pub roles: Vec<String>,
}
