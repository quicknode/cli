//! Config file (`~/.config/qn/config.toml`) load/save and API-key resolution.
//!
//! Resolution order, highest to lowest precedence:
//!   1. `--api-key` flag
//!   2. `QN_CLI__API_KEY` env var
//!   3. config file
//!
//! When all three fail we return [`CliError::NoApiKey`] which exits 4 with a
//! message directing the user to `qn auth login`. The `qn auth login` command
//! is the only place that prompts interactively; other commands never block
//! waiting for input.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::CliError;
use crate::output::Format;

const ENV_API_KEY: &str = "QN_CLI__API_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Flag,
    Env,
    ConfigFile,
    Prompt,
}

impl KeySource {
    pub fn label(self) -> &'static str {
        match self {
            KeySource::Flag => "--api-key flag",
            KeySource::Env => "QN_CLI__API_KEY env var",
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

/// Saves `api_key` to `path`, creating parent directories and writing 0600 perms on unix.
///
/// Preserves any existing `[output]` section by reading the current file first.
pub fn save_api_key(path: &Path, api_key: &str) -> Result<(), CliError> {
    let mut cfg = load_from(path)?.unwrap_or_default();
    cfg.api.key = Some(api_key.to_string());
    let text = toml::to_string_pretty(&cfg).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, text).map_err(|source| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source,
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

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

/// Resolves an API key per the documented precedence: flag > env > config file.
///
/// `allow_prompt` and `prompt` exist only so `qn auth login` can opt into the
/// interactive path. Regular commands pass `allow_prompt = false`; if the
/// three non-interactive sources fail they get `Err(NoApiKey)`.
///
/// `prompt` is supplied by the caller so tests can inject a deterministic
/// closure instead of touching the real terminal. In production
/// [`prompt_for_api_key`] is the implementation used by `qn auth login`.
pub fn resolve_api_key(
    flag: Option<&str>,
    env_key: Option<&str>,
    config_path: Option<&Path>,
    allow_prompt: bool,
    prompt: impl FnOnce() -> Result<String, CliError>,
) -> Result<(String, KeySource), CliError> {
    if let Some(k) = flag.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok((k.to_string(), KeySource::Flag));
    }
    if let Some(k) = env_key.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok((k.to_string(), KeySource::Env));
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

/// Reads `QN_CLI__API_KEY` from the environment.
pub fn read_env_api_key() -> Option<String> {
    std::env::var(ENV_API_KEY)
        .ok()
        .filter(|s| !s.trim().is_empty())
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
    fn flag_wins_over_everything() {
        let (k, src) =
            resolve_api_key(Some("from-flag"), Some("from-env"), None, true, fail_prompt).unwrap();
        assert_eq!(k, "from-flag");
        assert_eq!(src, KeySource::Flag);
    }

    #[test]
    fn env_wins_over_config_and_prompt() {
        let (k, src) = resolve_api_key(None, Some("from-env"), None, true, fail_prompt).unwrap();
        assert_eq!(k, "from-env");
        assert_eq!(src, KeySource::Env);
    }

    #[test]
    fn empty_flag_falls_through_to_env() {
        let (k, src) =
            resolve_api_key(Some("   "), Some("from-env"), None, true, fail_prompt).unwrap();
        assert_eq!(k, "from-env");
        assert_eq!(src, KeySource::Env);
    }

    #[test]
    fn config_used_when_no_flag_or_env() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_api_key(&path, "from-config").unwrap();

        let (k, src) = resolve_api_key(None, None, Some(&path), false, fail_prompt).unwrap();
        assert_eq!(k, "from-config");
        assert_eq!(src, KeySource::ConfigFile);
    }

    #[test]
    fn config_missing_file_falls_through_to_prompt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let (k, src) =
            resolve_api_key(None, None, Some(&path), true, || Ok("prompted".to_string())).unwrap();
        assert_eq!(k, "prompted");
        assert_eq!(src, KeySource::Prompt);
    }

    #[test]
    fn no_inputs_with_prompt_disabled_returns_no_api_key() {
        let err = resolve_api_key(None, None, None, false, fail_prompt).unwrap_err();
        assert!(matches!(err, CliError::NoApiKey));
    }

    #[test]
    fn malformed_config_file_surfaces_bad_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = not valid = toml\n[[[").unwrap();
        let err = resolve_api_key(None, None, Some(&path), false, fail_prompt).unwrap_err();
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
}
