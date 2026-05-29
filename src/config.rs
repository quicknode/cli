//! Config file (`~/.config/qn/config.toml`) load/save and API-key resolution.
//!
//! Resolution order, highest to lowest precedence:
//!   1. `--api-key` flag
//!   2. `QN_SDK__API_KEY` env var
//!   3. config file
//!   4. interactive prompt (TTY only, no --no-input)
//!
//! When all four fail we return [`CliError::NoApiKey`] which exits 4.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::errors::CliError;

const ENV_API_KEY: &str = "QN_SDK__API_KEY";

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
            KeySource::Env => "QN_SDK__API_KEY env var",
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
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ApiSection {
    pub key: Option<String>,
}

/// Returns the canonical config path (`~/.config/qn/config.toml` on Linux,
/// `~/Library/Application Support/qn/config.toml` on macOS).
///
/// Returns `None` only if the platform has no notion of a config dir — never
/// happens on the platforms we support but the directories crate is
/// theoretically fallible.
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "quicknode", "qn").map(|p| p.config_dir().join("config.toml"))
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
pub fn save_api_key(path: &Path, api_key: &str) -> Result<(), CliError> {
    let cfg = ConfigFile {
        api: ApiSection {
            key: Some(api_key.to_string()),
        },
    };
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

/// Resolves an API key per the documented precedence.
///
/// `prompt` is supplied by the caller so tests can inject a deterministic
/// closure instead of touching the real terminal. In production
/// [`prompt_for_api_key`] is the implementation.
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

/// Reads `QN_SDK__API_KEY` from the environment.
pub fn read_env_api_key() -> Option<String> {
    std::env::var(ENV_API_KEY)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Interactive prompt for an API key. Hidden input on the terminal.
pub fn prompt_for_api_key() -> Result<String, CliError> {
    use dialoguer::Password;
    Password::new()
        .with_prompt("QuickNode API key")
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
