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
/// zrip compression when the server actually serves it, and `compress_level`
/// picks the zrip level policy within the range the server advertises.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibfwConfig {
    /// Download compression for the embedded server: `"zrip"` (the SDK can
    /// decompress it) or `"none"` (default — safe for plain browser fetches,
    /// which advertise `zstd` and would otherwise receive undecodable zrip
    /// bytes). Values are parsed like libfw's `x-libfw-compress` header
    /// (`zrip`/`zstd`/`identity`/`none`).
    #[serde(default = "default_compression")]
    pub compression: String,    /// Client SDK: zrip compression-level policy (default `"balanced"`).
    ///
    /// Served to the browser as `libfw-client`'s `compressLevel` option, which
    /// clamps it into the zrip range the server advertises on
    /// `/capabilities`:
    /// - `"fast"` — the advertised minimum (least CPU, worst ratio)
    /// - `"balanced"` — the advertised default
    /// - `"max"` — the advertised maximum (best ratio)
    /// - a bare integer — that level, clamped into the advertised range
    /// - `"auto"` — additionally micro-benchmarks a real sample of the first
    ///   uploaded file against the range and picks the best
    ///   bytes-saved-vs-CPU trade-off (requires `auto_tune = true`).
    ///
    /// An unrecognised value falls back to `"balanced"` (with a startup
    /// warning). Only meaningful with `compression = "zrip"`: the SDK's
    /// `compress` master switch sends plain identity bodies otherwise.
    #[serde(default = "default_compress_level", deserialize_with = "de_string_or_int")]
    pub compress_level: String,    /// Upper bound for a single upload body in bytes (default 100 GiB).
    #[serde(default = "default_max_upload_size")]
    pub max_upload_size: u64,
    /// Client SDK: max parallel file transfers (default 4).
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    /// Client SDK: shared chunk size in bytes for upload chunks and
    /// parallel download byte ranges (default 2 MiB). libfw-client 0.4.4
    /// unified the former upload/download chunk-size knobs into this one.
    #[serde(default = "default_chunk_size")]
    pub chunk_size: u64,
    /// Client SDK: per-file scheduling window — how many chunks of the same
    /// file may be in flight at once (default 4, matching concurrency).
    ///
    /// Total in-flight upload requests ≈ `concurrency × upload_window`, so
    /// keeping this at the concurrency value bounds how "wild" an upload
    /// looks on the network panel. Raise it for deeper per-file pipelining.
    #[serde(default = "default_upload_window")]
    pub upload_window: u32,
    /// Client SDK: per-file download scheduling window — how many byte
    /// ranges of the same file may be fetched in parallel (default 4, the
    /// SDK's own default). Mirror of `upload_window` on the download side;
    /// without this knob the frontend would silently use the SDK default.
    #[serde(default = "default_download_window")]
    pub download_window: u32,
    /// Client SDK: retries per chunk/file before failing (default 3).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Client SDK: initial exponential-backoff delay (ms, default 500).
    #[serde(default = "default_base_retry_delay_ms")]
    pub base_retry_delay_ms: u64,
    /// Client SDK: backoff ceiling (ms, default 30 s).
    #[serde(default = "default_max_retry_delay_ms")]
    pub max_retry_delay_ms: u64,
    /// Client SDK: per-read (idle) timeout in ms (default 10 min).
    ///
    /// The libfw engine applies this as a PER-READ timeout on the transfer
    /// socket/HTTP response: it races every read against a `setTimeout` and
    /// ABORTS the whole transfer if a single read stalls longer than this. It
    /// is NOT a total-transfer deadline and it does not reset on activity
    /// within a read. A small value (e.g. the old 60 s default) therefore
    /// kills otherwise-healthy transfers on slow links, large uploads (server
    /// commit) and high-latency reconciliation, so it must be generous. `0`
    /// disables the JS timer (the browser's own socket/network error/close
    /// still surfaces a truly dead peer).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Client SDK: enable the adaptive tuning engine (default false).
    ///
    /// When enabled the browser probes the server's public `GET /capabilities`
    /// advertisement and TCP-style ramps the per-file window, then cross-file
    /// concurrency, from real transfer stats. The chunk size is not ramped: it
    /// follows the measured link (~100 ms of throughput, clamped into the
    /// advertised range), and the zrip level is a client policy resolved from
    /// the server's advertised levels. Static knobs above act as the
    /// starting/minimum values. Disabled by default so transfers behave
    /// exactly as configured.
    #[serde(default)]
    pub auto_tune: bool,
    /// Client SDK: chunk-size ceiling advertised to the adaptive engine, in
    /// bytes (default 8 MiB). Only used with `auto_tune = true`.
    ///
    /// With adaptive tuning the SDK does not ramp the chunk size — it sizes
    /// every request from the *measured* link (~100 ms of throughput) — so the
    /// advertised range becomes `[64 KiB, this ceiling]` and `chunk_size` stops
    /// acting as the ceiling. Raising it lets a wide/fast link use fewer,
    /// bigger requests; the SDK enforces its own hard cap (16 MiB) and an
    /// in-flight memory budget regardless, so values above 16 MiB have no
    /// effect. Ignored when `auto_tune = false` (the fixed `chunk_size` is then
    /// advertised as min, default and max alike).
    #[serde(default = "default_auto_tune_max_chunk_size")]
    pub auto_tune_max_chunk_size: u64,
    /// Client SDK: how long a settled tuning result stays usable, in ms
    /// (default 1 h).
    ///
    /// The browser caches a settle per origin *and* direction, so a page
    /// reload skips the ramp; the TTL counts from the moment the ramp settled.
    /// `0` disables the cache entirely (every transfer re-ramps). Only
    /// meaningful with `auto_tune = true`.
    #[serde(default = "default_tune_ttl_ms")]
    pub tune_ttl_ms: u64,
    /// AES-256 key (64 hex chars = 32 bytes) for libfw's `EncryptedPathCodec`.
    ///
    /// Required: real storage paths are encrypted into opaque
    /// `v1.<base64url>` shadow paths before anything reaches the browser
    /// (bearer tokens, `/file`/`/dir` URLs, directory listings, upload
    /// echoes), so no real filesystem path ever leaves the server. The
    /// server refuses to start without a valid key. Generate with
    /// `openssl rand -hex 32`; keep it stable across restarts (rotating it
    /// invalidates outstanding tokens, which expire within the 1 h TTL
    /// anyway).
    #[serde(default)]
    pub path_key: String,
}

impl Default for LibfwConfig {
    fn default() -> Self {
        LibfwConfig {
            compression: default_compression(),
            compress_level: default_compress_level(),
            max_upload_size: default_max_upload_size(),
            concurrency: default_concurrency(),
            chunk_size: default_chunk_size(),
            upload_window: default_upload_window(),
            download_window: default_download_window(),
            max_retries: default_max_retries(),
            base_retry_delay_ms: default_base_retry_delay_ms(),
            max_retry_delay_ms: default_max_retry_delay_ms(),
            timeout_ms: default_timeout_ms(),
            auto_tune: false,
            auto_tune_max_chunk_size: default_auto_tune_max_chunk_size(),
            tune_ttl_ms: default_tune_ttl_ms(),
            path_key: String::new(),
        }
    }
}

fn default_compression() -> String {
    "none".to_string()
}
fn default_compress_level() -> String {
    "balanced".to_string()
}
fn default_auto_tune_max_chunk_size() -> u64 {
    8 * 1024 * 1024 // 8 MiB — a fast link's "~100 ms of throughput" budget
}

/// Accept `compress_level = "balanced"` (string) as well as a bare integer
/// (`compress_level = 3`), normalising both to one string.
///
/// A `Visitor` rather than an untagged enum: it accepts whatever type TOML
/// actually parsed instead of buffering the value through `Content`.
fn de_string_or_int<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrInt;

    impl serde::de::Visitor<'_> for StringOrInt {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a zrip level policy name or a numeric level")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }
    }

    deserializer.deserialize_any(StringOrInt)
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
fn default_upload_window() -> u32 {
    4 // matches default concurrency, so uploads stay bounded
}
fn default_download_window() -> u32 {
    4 // matches the libfw-client SDK's own downloadWindow default
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
fn default_tune_ttl_ms() -> u64 {
    3_600_000 // 1 h — matches the libfw-client SDK's own tuneTtlMs default
}

impl LibfwConfig {
    /// Parse `compression` the same way libfw parses its `x-libfw-compress`
    /// header. Anything other than `zrip`/`zstd` falls back to `None`.
    pub fn compression_format(&self) -> CompressionFormat {
        CompressionFormat::parse_header(&self.compression).unwrap_or(CompressionFormat::None)
    }

    /// Whether `compress_level` is a policy libfw-client understands
    /// (`auto`/`fast`/`balanced`/`max`) or a numeric zrip level.
    pub fn compress_level_is_valid(&self) -> bool {
        compress_level_json(self.compress_level.trim()).is_some()
    }

    /// The `compressLevel` value served to the browser: a named policy, a
    /// numeric level, or `"balanced"` when the configured value is not
    /// recognised (libfw-client falls back to the same default, so this only
    /// keeps `config.js` well-formed instead of shipping junk).
    pub fn compress_level_json(&self) -> serde_json::Value {
        compress_level_json(self.compress_level.trim())
            .unwrap_or_else(|| serde_json::Value::String("balanced".to_string()))
    }

    /// Build libfw's `EncryptedPathCodec` from `path_key`.
    ///
    /// Fails (rather than silently falling back to identity) when the key is
    /// missing or malformed: an identity fallback would start leaking real
    /// paths to the browser, which is exactly what this codec exists to
    /// prevent.
    pub fn path_codec(&self) -> Result<libfw_core::pathmap::EncryptedPathCodec, String> {
        libfw_core::pathmap::EncryptedPathCodec::from_hex(&self.path_key)
            .map_err(|e| format!("[libfw] path_key: {e}"))
    }
}

/// Map a `compress_level` string to the JSON value `config.js` serves: the
/// lowercase policy name, a numeric zrip level, or `None` when unparseable.
fn compress_level_json(raw: &str) -> Option<serde_json::Value> {
    let lower = raw.to_ascii_lowercase();
    if matches!(lower.as_str(), "auto" | "fast" | "balanced" | "max") {
        return Some(serde_json::Value::String(lower));
    }
    raw.parse::<i32>().ok().map(|level| serde_json::json!(level))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in for the real `Config`: only the `[libfw]` table matters here.
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        libfw: LibfwConfig,
    }

    fn parse(toml_src: &str) -> LibfwConfig {
        toml::from_str::<Wrapper>(toml_src).unwrap().libfw
    }

    #[test]
    fn defaults_apply_without_a_libfw_table() {
        let cfg = parse("");
        assert_eq!(cfg.compress_level, "balanced");
        assert_eq!(cfg.auto_tune_max_chunk_size, 8 * 1024 * 1024);
        assert!(!cfg.auto_tune);
        assert_eq!(cfg.compress_level_json(), serde_json::json!("balanced"));
    }

    #[test]
    fn compress_level_accepts_a_policy_name_or_a_bare_integer() {
        // Quoted policy names are normalised to lowercase for the SDK.
        let named = parse("[libfw]\ncompress_level = \"MAX\"\n");
        assert_eq!(named.compress_level_json(), serde_json::json!("max"));
        assert!(named.compress_level_is_valid());

        // An unquoted integer is an explicit zrip level (and must not fail the
        // whole config parse).
        let numeric = parse("[libfw]\ncompress_level = -8\n");
        assert_eq!(numeric.compress_level_json(), serde_json::json!(-8));
        assert!(numeric.compress_level_is_valid());

        // Junk falls back to the SDK's own default instead of breaking
        // `config.js`.
        let junk = parse("[libfw]\ncompress_level = \"turbo\"\n");
        assert_eq!(junk.compress_level_json(), serde_json::json!("balanced"));
        assert!(!junk.compress_level_is_valid());
    }

    #[test]
    fn auto_tune_chunk_ceiling_is_parsed() {
        let cfg = parse("[libfw]\nauto_tune = true\nauto_tune_max_chunk_size = 4194304\n");
        assert!(cfg.auto_tune);
        assert_eq!(cfg.auto_tune_max_chunk_size, 4 * 1024 * 1024);
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
    /// Audit-log retention in days: entries older than this are pruned at
    /// startup and then hourly. `0` keeps entries forever. Default 365.
    #[serde(default = "default_audit_retention_days")]
    pub audit_retention_days: u32,
    /// Username of the instance administrator. When set, this user — matched
    /// against the OIDC `name`/`preferred_username` claim (the display name
    /// shown in the UI) or the OIDC `sub` — is the admin. Admin status is
    /// derived from the config on every request and is NOT stored in the
    /// database, so granting or revoking admin is a config edit (takes effect
    /// on the user's next request, no re-login needed). Leave unset to keep
    /// the legacy "first user to log in becomes admin" behavior.
    #[serde(default)]
    pub admin_user: Option<String>,
}

fn default_audit_retention_days() -> u32 {
    365
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// Optional: authorization endpoint override.
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub authorization_endpoint: Option<String>,
    /// Optional: token endpoint override.
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub token_endpoint: Option<String>,
    /// Optional: userinfo endpoint override.
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub userinfo_endpoint: Option<String>,
    /// Optional: JWKS URI override (ID token signature verification keys).
    /// If not set, discovered from {issuer_url}/.well-known/openid-configuration.
    pub jwks_uri: Option<String>,
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
