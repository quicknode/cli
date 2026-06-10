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
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ApiSection {
    pub key: Option<String>,
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
/// rather than the OS-native `directories`-crate locations.
///
/// Returns `None` only if neither `$XDG_CONFIG_HOME` nor `$HOME` is set, which
/// would mean the user's shell environment is broken.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("qn").join("config.toml"))
}

fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Pure version of [`config_dir`] for testing.
fn resolve_config_dir(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(xdg) = xdg_config_home {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p);
        }
    }
    home.map(|h| PathBuf::from(h).join(".config"))
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
    let text = toml::to_string_pretty(&cfg).map_err(|e| CliError::ConfigWrite {
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
        )
        .unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let d = resolve_config_dir(None, Some(std::ffi::OsString::from("/home/u"))).unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn config_dir_returns_none_without_home() {
        assert!(resolve_config_dir(None, None).is_none());
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
    fn delete_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        delete_config(&path).unwrap(); // no file yet
        save_api_key(&path, "k").unwrap();
        delete_config(&path).unwrap();
        delete_config(&path).unwrap(); // already gone
        assert!(!path.exists());
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
