//! libfw token issuer and verifier.
//!
//! Issues HMAC-signed bearer tokens that encode path + permission claims.
//! The companion [`OneshareTokenVerifier`] implements libfw's `TokenVerifier`
//! trait so the embedded libfw router can validate tokens independently.

use base64::Engine;
use hmac::{Hmac, Mac};
use libfw_core::auth::{Action, AuthError, TokenVerifier, Validator};
use libfw_core::claims::{Permission, TokenClaims};
use libfw_core::pathmap::PathCodec;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

/// The payload we sign inside every token.
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenPayload {
    pub sub: String,
    pub path: String,
    pub permissions: Vec<String>,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
}

/// Issue an HMAC-signed bearer token.
pub fn issue_token(
    hmac_key: &str,
    sub: &str,
    path: &str,
    permissions: &[&str],
    ttl_secs: u64,
) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let payload = TokenPayload {
        sub: sub.to_string(),
        path: path.to_string(),
        permissions: permissions.iter().map(|s| s.to_string()).collect(),
        iat: now,
        exp: now + ttl_secs,
        jti: uuid::Uuid::new_v4().to_string(),
    };

    let payload_json = serde_json::to_string(&payload).unwrap();
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(payload_json.as_bytes());

    let mut mac = HmacSha256::new_from_slice(hmac_key.as_bytes()).expect("Invalid HMAC key");
    mac.update(payload_b64.as_bytes());
    let sig = mac.finalize();
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sig.into_bytes());

    format!("{}.{}", payload_b64, sig_b64)
}

/// Verify an HMAC-signed token and return the embedded payload.
pub fn verify_token(hmac_key: &str, token: &str) -> Result<TokenPayload, String> {
    let (payload_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| "Invalid token format".to_string())?;

    // Verify HMAC
    let mut mac = HmacSha256::new_from_slice(hmac_key.as_bytes())
        .map_err(|e| format!("HMAC init error: {}", e))?;
    mac.update(payload_b64.as_bytes());

    let expected_sig = mac.finalize();
    let expected_sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(expected_sig.into_bytes());

    if sig_b64 != expected_sig_b64 {
        return Err("Token signature mismatch".to_string());
    }

    // Decode payload
    let payload_json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|e| format!("Base64 decode error: {}", e))?;

    let payload: TokenPayload = serde_json::from_slice(&payload_json)
        .map_err(|e| format!("JSON parse error: {}", e))?;

    // Check expiry
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if now >= payload.exp {
        return Err("Token expired".to_string());
    }

    Ok(payload)
}

// ── libfw TokenVerifier impl ──

#[derive(Clone)]
pub struct OneshareTokenVerifier {
    pub hmac_key: String,
}

impl TokenVerifier for OneshareTokenVerifier {
    fn verify(&self, token: &str) -> Result<TokenClaims, AuthError> {
        let payload = verify_token(&self.hmac_key, token)
            .map_err(|e| AuthError::Invalid(e))?;

        let permissions: Vec<Permission> = payload
            .permissions
            .iter()
            .filter_map(|p| match p.as_str() {
                "read" => Some(Permission::Read),
                "write" => Some(Permission::Write),
                _ => None,
            })
            .collect();

        Ok(TokenClaims {
            sub: payload.sub,
            exp: Some(payload.exp as i64),
            permissions,
            allowed_paths: vec![payload.path],
        })
    }
}

// ── libfw Validator impl (shadow-path tokens) ──

/// Validates libfw requests whose tokens are bound to *opaque shadow paths*.
///
/// `/api/files/token` now issues tokens carrying `EncryptedPathCodec` shadows
/// (`v1.<base64url>`), never real paths, so the real filesystem path is
/// absent from everything the browser holds. libfw resolves the request
/// URL's shadow back to the real path and calls this validator with it; we
/// decode each `allowed_path` claim back to its real path and match with
/// libfw's usual segment-boundary prefix semantics.
///
/// A claim that fails to decode (e.g. a legacy token that still embeds a
/// plain real path) is matched literally, so tokens issued before the codec
/// was enabled keep working until they expire.
pub struct CodecPathValidator {
    codec: Arc<dyn PathCodec>,
    /// Mirrors `libfw_core::auth::PathValidator::raw_prefix_match`: when
    /// true, allowed paths are treated as raw string prefixes instead of
    /// segment-boundary prefixes. Default false.
    pub raw_prefix_match: bool,
}

impl CodecPathValidator {
    pub fn new(codec: Arc<dyn PathCodec>) -> Self {
        CodecPathValidator {
            codec,
            raw_prefix_match: false,
        }
    }
}

impl Validator for CodecPathValidator {
    fn validate(&self, claims: &TokenClaims, path: &str, action: Action) -> Result<(), AuthError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if claims.is_expired(now) {
            return Err(AuthError::Expired);
        }
        if !claims.has_permission(action.required_permission()) {
            return Err(AuthError::Forbidden {
                path: path.to_string(),
                action,
            });
        }

        // Claims carry shadows; decode them back to real paths before
        // matching. Undecodable claims (legacy plain paths, tampered
        // shadows) fall back to literal matching — a tampered shadow never
        // equals a real path, so that direction still fails closed.
        let allowed: Vec<String> = claims
            .allowed_paths
            .iter()
            .map(|claim| match self.codec.decode(claim) {
                Ok(real) => real,
                Err(_) => claim.clone(),
            })
            .collect();

        let allowed = if self.raw_prefix_match {
            allowed.iter().any(|prefix| path.starts_with(prefix.as_str()))
        } else {
            path_matches_any(path, &allowed)
        };
        if !allowed {
            return Err(AuthError::Forbidden {
                path: path.to_string(),
                action,
            });
        }
        Ok(())
    }
}

/// Segment-boundary prefix matching, identical to libfw-core's
/// `PathValidator` semantics: `a/b` matches `a/b` and `a/b/c` but not
/// `a/bc`; the empty prefix grants the whole tree.
fn path_matches_any(path: &str, prefixes: &[String]) -> bool {
    let p = path.trim_start_matches('/');
    prefixes.iter().any(|prefix| {
        let q = prefix.trim_matches('/');
        if q.is_empty() {
            // Root prefix → grant access to the whole tree.
            return true;
        }
        if p == q {
            return true;
        }
        p.starts_with(q) && p.as_bytes().get(q.len()) == Some(&b'/')
    })
}
