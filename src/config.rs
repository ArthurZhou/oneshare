use libfw_core::compress::CompressionFormat;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub oidc: OidcConfig,
    /// libfw file-transfer configuration. Every field has a default, so an
    /// existing `config.toml` without a `[libfw]` table keeps working.
    #[serde(default)]
    pub libfw: LibfwConfig,
}

/// libfw transfer settings.
///
/// Two kinds of knobs live here:
/// - **Server** knobs applied to the embedded libfw router
///   (`compression`, `max_upload_size`).
/// - **Client** knobs served to the browser via `config.js`
///   (`window.ONESHARE_LIBFW`) so the frontend configures the `libfw-client`
///   SDK from the backend instead of hard-coding them.
///
/// `compress` (client) mirrors `compression` (server): the SDK only negotiates
/// zrip compression when the server actually serves it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibfwConfig {
    /// Download compression for the embedded server: `"zrip"` (the SDK can
    /// decompress it) or `"none"` (default — safe for plain browser fetches,
    /// which advertise `zstd` and would otherwise receive undecodable zrip
    /// bytes). Values are parsed like libfw's `x-libfw-compress` header
    /// (`zrip`/`zstd`/`identity`/`none`).
    #[serde(default = "default_compression")]
    pub compression: String,
    /// Upper bound for a single upload body in bytes (default 100 GiB).
    #[serde(default = "default_max_upload_size")]
    pub max_upload_size: u64,
    /// Client SDK: max parallel file transfers (default 4).
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    /// Client SDK: upload chunk size in bytes (default 2 MiB).
    #[serde(default = "default_chunk_size")]
    pub chunk_size: u64,
    /// Client SDK: retries per chunk/file before failing (default 3).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Client SDK: initial exponential-backoff delay (ms, default 500).
    #[serde(default = "default_base_retry_delay_ms")]
    pub base_retry_delay_ms: u64,
    /// Client SDK: backoff ceiling (ms, default 30 s).
    #[serde(default = "default_max_retry_delay_ms")]
    pub max_retry_delay_ms: u64,
    /// Client SDK: WebSocket per-read (idle) timeout in ms (default 10 min).
    ///
    /// The libfw engine applies this as a PER-READ timeout on the transfer
    /// socket: `WsConnection::next()` races every frame read against a
    /// `setTimeout` and ABORTS the whole transfer if a single read stalls
    /// longer than this. It is NOT a total-transfer deadline and it does not
    /// reset on activity within a read. A small value (e.g. the old 60 s
    /// default) therefore kills otherwise-healthy transfers on slow links,
    /// large uploads (server commit) and high-latency wave reconciliations,
    /// so it must be generous. `0` disables the JS timer (the browser's own
    /// socket error/close still surfaces a truly dead peer).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for LibfwConfig {
    fn default() -> Self {
        LibfwConfig {
            compression: default_compression(),
            max_upload_size: default_max_upload_size(),
            concurrency: default_concurrency(),
            chunk_size: default_chunk_size(),
            max_retries: default_max_retries(),
            base_retry_delay_ms: default_base_retry_delay_ms(),
            max_retry_delay_ms: default_max_retry_delay_ms(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

fn default_compression() -> String {
    "none".to_string()
}
fn default_max_upload_size() -> u64 {
    100 * 1024 * 1024 * 1024 // 100 GiB, matches libfw's DEFAULT_MAX_UPLOAD_SIZE
}
fn default_concurrency() -> u32 {
    4
}
fn default_chunk_size() -> u64 {
    2 * 1024 * 1024 // 2 MiB
}
fn default_max_retries() -> u32 {
    3
}
fn default_base_retry_delay_ms() -> u64 {
    500
}
fn default_max_retry_delay_ms() -> u64 {
    30_000
}
fn default_timeout_ms() -> u64 {
    600_000 // 10 min — generous per-read timeout so active transfers aren't aborted
}

impl LibfwConfig {
    /// Parse `compression` the same way libfw parses its `x-libfw-compress`
    /// header. Anything other than `zrip`/`zstd` falls back to `None`.
    pub fn compression_format(&self) -> CompressionFormat {
        CompressionFormat::parse_header(&self.compression).unwrap_or(CompressionFormat::None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub listen_port: u16,
    pub root_dir: PathBuf,
    pub database_url: String,
    pub hmac_secret: String,
    /// Optional URL prefix (base path) so OneShare can be served behind a
    /// reverse proxy under a sub-path of a shared domain, e.g. "/oneshare".
    /// Empty or "/" means the app is served at the domain root.
    #[serde(default)]
    pub base_url: String,
    /// Mark the session cookie `Secure` (HTTPS-only). Enable when serving over
    /// TLS (e.g. behind an HTTPS reverse proxy). Default `false` for local HTTP.
    #[serde(default)]
    pub session_cookie_secure: bool,
    /// Optional list of origins allowed to make cross-origin requests (CORS).
    /// Empty (default) = same-origin only; cross-origin browser requests are
    /// blocked. The frontend is served by this app, so leave empty unless a
    /// separate origin really needs to call the API.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Trash directory for deletions. When non-empty, deleting a file/folder
    /// MOVES it into this directory (preserving its relative path) instead of
    /// deleting it permanently, so it can be recovered. Empty (default) =
    /// delete permanently.
    ///
    /// Relative paths are resolved against `root_dir`; `.trash` is a good
    /// choice because dot-prefixed entries are hidden from listings. Absolute
    /// paths are used as-is.
    #[serde(default)]
    pub trash_dir: String,
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

    pub fn hmac_secret(&self) -> &str {
        &self.server.hmac_secret
    }

    /// The resolved trash directory, or `None` when `trash_dir` is empty/blank
    /// (i.e. deletions are permanent). Relative paths resolve against
    /// `root_dir`; absolute paths are used as-is.
    pub fn trash_path(&self) -> Option<PathBuf> {
        let t = self.server.trash_dir.trim();
        if t.is_empty() {
            None
        } else {
            let p = PathBuf::from(t);
            Some(if p.is_absolute() {
                p
            } else {
                self.server.root_dir.join(p)
            })
        }
    }

    /// Normalized URL prefix (base path). Empty string (or "/") means the app
    /// is served at the domain root; otherwise a leading "/" is added and any
    /// trailing "/" removed, e.g. "/oneshare". Never returns a trailing slash.
    pub fn base_url(&self) -> String {
        let b = self.server.base_url.trim();
        if b.is_empty() || b == "/" {
            String::new()
        } else {
            format!("/{}", b.trim_matches('/'))
        }
    }

    /// Absolute path to redirect the browser to after login/logout. At the
    /// root prefix this is "/"; under a prefix it is the prefix itself, so the
    /// browser lands back on the app's home page, e.g. "/oneshare".
    pub fn redirect_after_auth(&self) -> String {
        let base = self.base_url();
        if base.is_empty() {
            "/".to_string()
        } else {
            base
        }
    }
}
