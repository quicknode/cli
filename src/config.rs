//! Config file (`~/.config/qn/config.toml`) load/save and API-key resolution.
//!
//! Resolution order, highest to lowest precedence:
//!   1. `--api-key` flag
//!   2. config file (`--config-file` path if given, else the default path)
//!
//! There is deliberately no environment-variable source: a key left exported
//! in a shell is invisible state that outlives the session it was set for,
//! and is the easiest way to run a destructive command against the wrong
//! account.
//!
//! When both sources fail we return [`CliError::NoApiKey`] which exits 4 with
//! a message directing the user to `qn auth login`. The `qn auth login`
//! command is the only place that prompts interactively; other commands never
//! block waiting for input.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::CliError;
use crate::output::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Flag,
    ConfigFile,
    Prompt,
}

impl KeySource {
    pub fn label(self) -> &'static str {
        match self {
            KeySource::Flag => "--api-key flag",
            KeySource::ConfigFile => "config file",
            KeySource::Prompt => "interactive prompt",
        }
    }
}

/// On-disk shape of `~/.config/qn/config.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub api: ApiSection,
    #[serde(default)]
    pub output: OutputSection,
    #[serde(default)]
    pub rpc: RpcSection,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ApiSection {
    pub key: Option<String>,
}

/// `[rpc]` section: RPC-specific defaults. `endpoint_url`, when set, routes
/// `qn rpc call` at a fully-formed custom HTTP URL instead of the account's
/// Tooling Access endpoint (self-authenticating: no JWT minted). A per-call
/// `--endpoint-url` overrides it. Mirrors the SDK's `RpcConfig.endpoint_url`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RpcSection {
    #[serde(default)]
    pub endpoint_url: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OutputSection {
    /// Default output format when the CLI is invoked without `--format`. One
    /// of: `table`, `json`, `yaml`, `md`, `toon`.
    #[serde(default)]
    pub format: Option<Format>,
    /// Default for `--wide`. CLI flag still overrides — `--wide` alone toggles
    /// it on, but there's no `--no-wide` (use `--format json` if you need full
    /// data without the wide setting).
    #[serde(default)]
    pub wide: bool,
}

/// Returns the canonical config path: `$XDG_CONFIG_HOME/qn/config.toml` if the
/// env var is set, otherwise `~/.config/qn/config.toml`. We use the same path
/// on every platform — easier to document, easier to share across machines —
/// rather than the OS-native `directories`-crate locations. The home directory
/// comes from `$HOME`, falling back to `%USERPROFILE%` (Windows shells set the
/// latter, not the former).
///
/// Returns `None` only if none of `$XDG_CONFIG_HOME`, `$HOME`, or
/// `%USERPROFILE%` is set, which would mean the user's shell environment is
/// broken.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("qn").join("config.toml"))
}

fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
    )
}

/// Pure version of [`config_dir`] for testing.
fn resolve_config_dir(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(xdg) = xdg_config_home {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p);
        }
    }
    home.or(userprofile)
        .map(|h| PathBuf::from(h).join(".config"))
}

/// Loads the config file at `path`, returning `Ok(None)` if it doesn't exist.
pub fn load_from(path: &Path) -> Result<Option<ConfigFile>, CliError> {
    let text = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let cfg = toml::from_str::<ConfigFile>(&text).map_err(|source| CliError::BadConfig {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(cfg))
}

/// Saves `api_key` to `path` atomically, with 0600 perms on Unix.
///
/// - Preserves any existing `[output]` section by reading the current file first.
/// - Writes via a temp file in the same directory and `rename`s over the target,
///   so a crash mid-write can never leave a truncated config behind.
/// - On Unix, the temp file gets 0600 perms BEFORE the secret bytes are written
///   (so the key is never world-readable, not even briefly), and the parent
///   directory is best-effort tightened to 0700.
pub fn save_api_key(path: &Path, api_key: &str) -> Result<(), CliError> {
    let mut cfg = load_from(path)?.unwrap_or_default();
    cfg.api.key = Some(api_key.to_string());
    write_config(path, &cfg)
}

/// Removes the saved API key while preserving the rest of the config (output
/// preferences). Used by `qn auth logout` so logging out doesn't reset
/// `[output]` settings the user deliberately chose.
pub fn clear_api_key(path: &Path) -> Result<(), CliError> {
    let mut cfg = load_from(path)?.unwrap_or_default();
    cfg.api.key = None;
    write_config(path, &cfg)
}

/// Atomically writes `cfg` to `path`: serialize, write to a 0600 tempfile in the
/// same directory, fsync, then rename into place. Shared by [`save_api_key`] and
/// [`clear_api_key`].
fn write_config(path: &Path, cfg: &ConfigFile) -> Result<(), CliError> {
    let text = toml::to_string_pretty(cfg).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;

    let parent = path.parent().ok_or_else(|| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other("config path has no parent directory"),
    })?;
    fs::create_dir_all(parent).map_err(|source| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source,
    })?;

    // Best-effort: tighten the parent directory perms. Users may already have
    // a directory with different perms, and we don't want to refuse the save
    // over a directory chmod failure.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }

    let mut tmp = tempfile::Builder::new()
        .prefix(".qn-config-")
        .tempfile_in(parent)
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;

    // Set 0600 BEFORE writing the secret. The umask default (often 0644) would
    // otherwise leave a brief window where the key is world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600)).map_err(|source| {
            CliError::ConfigWrite {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }

    use std::io::Write;
    tmp.as_file_mut()
        .write_all(text.as_bytes())
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;

    tmp.persist(path).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: e.error,
    })?;

    Ok(())
}

// ── Tooling Access token cache ───────────────────────────────────────────────
//
// Each `qn rpc` is a fresh process, so the SDK's in-memory JWT cache starts
// empty every time. We persist the short-lived (~10 min) session token next to
// the config (`tokens.toml`) and re-seed the SDK on the next invocation,
// avoiding a control-plane round trip while the token is still valid.
//
// Only the short-lived JWT is written here — never the long-lived API key. The
// entry is scoped to the account by a fingerprint (SHA-256) of the API key, so
// switching keys transparently invalidates a stale token rather than presenting
// one account's JWT to another's endpoint.

use quicknode_sdk::CachedToken;

/// On-disk shape of `~/.config/qn/tokens.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TokenCacheFile {
    #[serde(default)]
    pub token: Option<CachedTokenEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTokenEntry {
    /// SHA-256 hex of the API key this token was minted for. Never the key.
    pub key_hash: String,
    pub endpoint_url: String,
    pub token: String,
    pub exp_unix: i64,
}

/// The token cache path: `tokens.toml` alongside the resolved config file (so
/// `--config-file` keeps config and tokens together). Falls back to the default
/// config dir when no explicit config path is given.
pub fn token_cache_path(config_path: Option<&Path>) -> Option<PathBuf> {
    match config_path {
        Some(p) => p.parent().map(|d| d.join("tokens.toml")),
        None => config_dir().map(|d| d.join("qn").join("tokens.toml")),
    }
}

/// Hex SHA-256 of the API key, used to scope a cached token to its account.
pub fn fingerprint_key(api_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(api_key.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Loads a cached token for `api_key` from `path`. Returns `None` if the file is
/// absent, unparseable, empty, or scoped to a different key (account switch).
/// A malformed cache is treated as a miss, never an error — the SDK will mint.
pub fn load_token(path: &Path, api_key: &str) -> Option<CachedToken> {
    let text = fs::read_to_string(path).ok()?;
    let cache: TokenCacheFile = toml::from_str(&text).ok()?;
    let entry = cache.token?;
    if entry.key_hash != fingerprint_key(api_key) {
        return None;
    }
    Some(CachedToken {
        endpoint_url: entry.endpoint_url,
        token: entry.token,
        exp_unix: entry.exp_unix,
    })
}

/// Saves `token` to `path` atomically with 0600 perms, scoped to `api_key`.
/// Mirrors [`save_api_key`]'s write discipline: temp file in the same dir,
/// 0600 set before the secret bytes, `rename` over the target. Last-write-wins
/// under concurrency — two `qn rpc` processes may both mint, but the atomic
/// rename guarantees no partial file and both tokens are valid.
pub fn save_token(path: &Path, api_key: &str, token: &CachedToken) -> Result<(), CliError> {
    let cache = TokenCacheFile {
        token: Some(CachedTokenEntry {
            key_hash: fingerprint_key(api_key),
            endpoint_url: token.endpoint_url.clone(),
            token: token.token.clone(),
            exp_unix: token.exp_unix,
        }),
    };
    let text = toml::to_string_pretty(&cache).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;

    let parent = path.parent().ok_or_else(|| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other("token cache path has no parent directory"),
    })?;
    fs::create_dir_all(parent).map_err(|source| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source,
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }

    let mut tmp = tempfile::Builder::new()
        .prefix(".qn-tokens-")
        .tempfile_in(parent)
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600)).map_err(|source| {
            CliError::ConfigWrite {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }

    use std::io::Write;
    tmp.as_file_mut()
        .write_all(text.as_bytes())
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    tmp.persist(path).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

// ── Multichain network URL cache ─────────────────────────────────────────────
//
// The per-network URL map (network key -> http_url) is stable endpoint metadata,
// unlike the ~10-min JWT. We cache it separately in `networks.toml` with a
// 24-hour TTL so it isn't rewritten on every token refresh. Scoped to the
// endpoint id; re-fetched (via get_endpoint_urls) when absent or stale.

/// Seconds the cached network map is considered fresh (24h).
pub const NETWORKS_TTL_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NetworksCacheFile {
    #[serde(default)]
    pub entry: Option<NetworksEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworksEntry {
    /// Endpoint id the map belongs to; a different id is a cache miss.
    pub endpoint_id: String,
    /// Unix seconds the map was fetched, for the TTL check.
    pub fetched_at_unix: i64,
    /// network key -> full http_url.
    pub networks: std::collections::HashMap<String, String>,
}

/// The networks cache path: `networks.toml` alongside the resolved config file.
pub fn networks_cache_path(config_path: Option<&Path>) -> Option<PathBuf> {
    match config_path {
        Some(p) => p.parent().map(|d| d.join("networks.toml")),
        None => config_dir().map(|d| d.join("qn").join("networks.toml")),
    }
}

/// Loads the cached network map for `endpoint_id` from `path`, if present, for
/// the same endpoint, and fetched within the TTL (relative to `now_unix`).
/// Returns `None` (a cache miss) on any mismatch or parse failure.
pub fn load_networks(
    path: &Path,
    endpoint_id: &str,
    now_unix: i64,
) -> Option<std::collections::HashMap<String, String>> {
    let text = fs::read_to_string(path).ok()?;
    let cache: NetworksCacheFile = toml::from_str(&text).ok()?;
    let entry = cache.entry?;
    if entry.endpoint_id != endpoint_id {
        return None;
    }
    if now_unix.saturating_sub(entry.fetched_at_unix) >= NETWORKS_TTL_SECS {
        return None;
    }
    Some(entry.networks)
}

/// Saves the network map for `endpoint_id` to `path` atomically with 0600 perms,
/// stamping `fetched_at_unix` for the TTL check. Same write discipline as
/// [`save_token`].
pub fn save_networks(
    path: &Path,
    endpoint_id: &str,
    fetched_at_unix: i64,
    networks: &std::collections::HashMap<String, String>,
) -> Result<(), CliError> {
    let cache = NetworksCacheFile {
        entry: Some(NetworksEntry {
            endpoint_id: endpoint_id.to_string(),
            fetched_at_unix,
            networks: networks.clone(),
        }),
    };
    let text = toml::to_string_pretty(&cache).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;
    write_atomic_0600(path, text.as_bytes(), ".qn-networks-")
}

/// Atomically writes `bytes` to `path` with 0600 perms via a temp file in the
/// same directory (perms set before the bytes), then `rename`. Shared by the
/// token and networks caches.
fn write_atomic_0600(path: &Path, bytes: &[u8], tmp_prefix: &str) -> Result<(), CliError> {
    let parent = path.parent().ok_or_else(|| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other("cache path has no parent directory"),
    })?;
    fs::create_dir_all(parent).map_err(|source| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let mut tmp = tempfile::Builder::new()
        .prefix(tmp_prefix)
        .tempfile_in(parent)
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600)).map_err(|source| {
            CliError::ConfigWrite {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }
    use std::io::Write;
    tmp.as_file_mut()
        .write_all(bytes)
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    tmp.persist(path).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

/// Deletes the saved config file. No error if it didn't exist.
pub fn delete_config(path: &Path) -> Result<(), CliError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Resolves an API key per the documented precedence: flag > config file.
///
/// `allow_prompt` and `prompt` exist only so `qn auth login` can opt into the
/// interactive path. Regular commands pass `allow_prompt = false`; if the
/// non-interactive sources fail they get `Err(NoApiKey)`.
///
/// `prompt` is supplied by the caller so tests can inject a deterministic
/// closure instead of touching the real terminal. In production
/// [`prompt_for_api_key`] is the implementation used by `qn auth login`.
pub fn resolve_api_key(
    flag: Option<&str>,
    config_path: Option<&Path>,
    allow_prompt: bool,
    prompt: impl FnOnce() -> Result<String, CliError>,
) -> Result<(String, KeySource), CliError> {
    if let Some(k) = flag.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok((k.to_string(), KeySource::Flag));
    }
    if let Some(path) = config_path {
        if let Some(cfg) = load_from(path)? {
            if let Some(k) = cfg
                .api
                .key
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Ok((k.to_string(), KeySource::ConfigFile));
            }
        }
    }
    if allow_prompt {
        let key = prompt()?;
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return Ok((trimmed.to_string(), KeySource::Prompt));
        }
    }
    Err(CliError::NoApiKey)
}

/// True when both stdin and stderr are TTYs. We prompt only when both are —
/// stdin so the user can type, stderr so the prompt is visible even if stdout
/// is being piped.
pub fn can_prompt() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Interactive prompt for an API key. Hidden input on the terminal.
pub fn prompt_for_api_key() -> Result<String, CliError> {
    use dialoguer::Password;
    Password::new()
        .with_prompt("Quicknode API key")
        .interact()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fail_prompt() -> Result<String, CliError> {
        panic!("prompt should not be invoked")
    }

    #[test]
    fn flag_wins_over_config_and_prompt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "from-config").unwrap();

        let (k, src) = resolve_api_key(Some("from-flag"), Some(&path), true, fail_prompt).unwrap();
        assert_eq!(k, "from-flag");
        assert_eq!(src, KeySource::Flag);
    }

    #[test]
    fn empty_flag_falls_through_to_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "from-config").unwrap();

        let (k, src) = resolve_api_key(Some("   "), Some(&path), false, fail_prompt).unwrap();
        assert_eq!(k, "from-config");
        assert_eq!(src, KeySource::ConfigFile);
    }

    #[test]
    fn config_used_when_no_flag() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "from-config").unwrap();

        let (k, src) = resolve_api_key(None, Some(&path), false, fail_prompt).unwrap();
        assert_eq!(k, "from-config");
        assert_eq!(src, KeySource::ConfigFile);
    }

    #[test]
    fn config_missing_file_falls_through_to_prompt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let (k, src) =
            resolve_api_key(None, Some(&path), true, || Ok("prompted".to_string())).unwrap();
        assert_eq!(k, "prompted");
        assert_eq!(src, KeySource::Prompt);
    }

    #[test]
    fn no_inputs_with_prompt_disabled_returns_no_api_key() {
        let err = resolve_api_key(None, None, false, fail_prompt).unwrap_err();
        assert!(matches!(err, CliError::NoApiKey));
    }

    #[test]
    fn malformed_config_file_surfaces_bad_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = not valid = toml\n[[[").unwrap();
        let err = resolve_api_key(None, Some(&path), false, fail_prompt).unwrap_err();
        assert!(matches!(err, CliError::BadConfig { .. }), "got: {err:?}");
    }

    #[test]
    fn save_then_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        save_api_key(&path, "round-trip-key").unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.api.key.as_deref(), Some("round-trip-key"));
    }

    #[test]
    fn config_dir_uses_xdg_when_absolute() {
        let d = resolve_config_dir(
            Some(std::ffi::OsString::from("/custom/xdg")),
            Some(std::ffi::OsString::from("/home/u")),
            None,
        )
        .unwrap();
        assert_eq!(d, PathBuf::from("/custom/xdg"));
    }

    #[test]
    fn config_dir_ignores_relative_xdg() {
        // A non-absolute XDG_CONFIG_HOME is bogus per the spec; fall through to HOME.
        let d = resolve_config_dir(
            Some(std::ffi::OsString::from("relative/path")),
            Some(std::ffi::OsString::from("/home/u")),
            None,
        )
        .unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let d = resolve_config_dir(None, Some(std::ffi::OsString::from("/home/u")), None).unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn config_dir_falls_back_to_userprofile_without_home() {
        // Windows shells set USERPROFILE, not HOME.
        let userprofile = std::ffi::OsString::from(r"C:\Users\u");
        let d = resolve_config_dir(None, None, Some(userprofile.clone())).unwrap();
        assert_eq!(d, PathBuf::from(userprofile).join(".config"));
    }

    #[test]
    fn config_dir_prefers_home_over_userprofile() {
        // e.g. Git Bash on Windows sets both; HOME wins.
        let d = resolve_config_dir(
            None,
            Some(std::ffi::OsString::from("/home/u")),
            Some(std::ffi::OsString::from(r"C:\Users\u")),
        )
        .unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn config_dir_returns_none_without_home_or_userprofile() {
        assert!(resolve_config_dir(None, None, None).is_none());
    }

    #[test]
    fn output_format_round_trips_through_config_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[api]\nkey = \"k\"\n\n[output]\nformat = \"yaml\"\n").unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.api.key.as_deref(), Some("k"));
        assert_eq!(cfg.output.format, Some(Format::Yaml));
    }

    #[test]
    fn output_wide_round_trips_through_config_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[output]\nwide = true\n").unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert!(cfg.output.wide);
    }

    #[test]
    fn rpc_endpoint_url_round_trips_through_config_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[api]\nkey = \"k\"\n\n[rpc]\nendpoint_url = \"https://my-endpoint.example/rpc\"\n",
        )
        .unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(
            cfg.rpc.endpoint_url.as_deref(),
            Some("https://my-endpoint.example/rpc")
        );
    }

    #[test]
    fn rpc_section_is_optional() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[api]\nkey = \"k\"\n").unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.rpc.endpoint_url, None);
    }

    #[test]
    fn save_api_key_preserves_rpc_section() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[rpc]\nendpoint_url = \"https://x/rpc\"\n").unwrap();
        save_api_key(&path, "new-key").unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.api.key.as_deref(), Some("new-key"));
        assert_eq!(cfg.rpc.endpoint_url.as_deref(), Some("https://x/rpc"));
    }

    #[test]
    fn output_section_is_optional() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[api]\nkey = \"k\"\n").unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.output.format, None);
    }

    #[test]
    fn save_api_key_preserves_output_section() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[output]\nformat = \"toon\"\n").unwrap();
        save_api_key(&path, "new-key").unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.api.key.as_deref(), Some("new-key"));
        assert_eq!(cfg.output.format, Some(Format::Toon));
    }

    #[test]
    fn clear_api_key_preserves_output_section() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[api]\nkey = \"k\"\n\n[output]\nformat = \"json\"\nwide = true\n",
        )
        .unwrap();
        clear_api_key(&path).unwrap();
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.api.key, None);
        assert_eq!(cfg.output.format, Some(Format::Json));
        assert!(cfg.output.wide);
    }

    #[test]
    fn clear_api_key_with_no_existing_file_writes_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        clear_api_key(&path).unwrap();
        assert!(path.exists());
        let cfg = load_from(&path).unwrap().unwrap();
        assert_eq!(cfg.api.key, None);
    }

    #[cfg(unix)]
    #[test]
    fn clear_api_key_writes_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "secret").unwrap();
        clear_api_key(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn save_api_key_writes_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "secret").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn save_api_key_leaves_no_temp_files_on_success() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "secret").unwrap();
        let leftover: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".qn-config-"))
            .collect();
        assert!(
            leftover.is_empty(),
            "found tempfile leftovers: {leftover:?}"
        );
    }
}
