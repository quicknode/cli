//! `qn auth {login,logout,whoami,status}` — manage the CLI's stored API key.
//!
//! Login: prompts for the key (hidden input), writes `~/.config/qn/config.toml`.
//! Logout: deletes that file.
//! Whoami: shows where the resolved key came from, and confirms it works by
//! calling a low-cost API (chain list).
//! Status: same as whoami minus the network round-trip.

use std::io::{IsTerminal, Write};

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::{QuicknodeSdk, SdkFullConfig};

use crate::config::{self, KeySource};
use crate::context::GlobalArgs;
use crate::errors::CliError;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn auth login                       # prompts for the key (hidden input)\n  \
    qn auth login --api-key <KEY>       # non-interactive (e.g. CI)\n  \
    qn auth whoami                      # verify the key against the API\n  \
    qn --config-file ./ci.toml auth status")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: AuthCmd,
}

#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// Prompt for an API key and save it to ~/.config/qn/config.toml.
    Login(LoginArgs),
    /// Delete the saved API key.
    Logout,
    /// Show where the API key would come from, and confirm it works against the API.
    Whoami,
    /// Show where the API key would come from (no network call).
    Status,
}

#[derive(Debug, ClapArgs)]
pub struct LoginArgs {
    /// Provide the API key directly instead of being prompted. Useful for CI.
    #[arg(long)]
    pub api_key: Option<String>,
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        AuthCmd::Login(la) => login(la, global).await,
        AuthCmd::Logout => logout(global),
        AuthCmd::Whoami => whoami(global).await,
        AuthCmd::Status => status(global),
    }
}

async fn login(args: LoginArgs, global: GlobalArgs) -> Result<(), CliError> {
    let path = global.resolve_config_path().ok_or_else(|| {
        CliError::Arg("no config directory available on this platform".to_string())
    })?;

    let key = match args.api_key.or(global.api_key) {
        Some(k) => k.trim().to_string(),
        None => {
            if !config::can_prompt() || global.no_input {
                return Err(CliError::Arg(
                    "no TTY available; pass --api-key to log in non-interactively".to_string(),
                ));
            }
            config::prompt_for_api_key()?
        }
    };

    if key.is_empty() {
        return Err(CliError::Arg("API key cannot be empty".to_string()));
    }

    // Quick validation against the API so we don't silently save a bogus key.
    let sdk = QuicknodeSdk::new(&SdkFullConfig::from_api_key(key.clone()))?;
    crate::retry::retrying(global.retries, || sdk.admin.list_chains()).await?;

    config::save_api_key(&path, &key)?;
    if !global.quiet {
        let _ = writeln!(std::io::stderr(), "✓ Saved API key to {}", path.display());
    }
    Ok(())
}

fn logout(global: GlobalArgs) -> Result<(), CliError> {
    let path = global.resolve_config_path().ok_or_else(|| {
        CliError::Arg("no config directory available on this platform".to_string())
    })?;
    config::delete_config(&path)?;
    if !global.quiet {
        let _ = writeln!(std::io::stderr(), "✓ Removed saved API key");
    }
    Ok(())
}

fn status(global: GlobalArgs) -> Result<(), CliError> {
    let (key, source) = resolve_non_interactive(&global)?;
    print_status(&global, source, &redact(&key), None);
    Ok(())
}

async fn whoami(global: GlobalArgs) -> Result<(), CliError> {
    let (key, source) = resolve_non_interactive(&global)?;
    let redacted = redact(&key);
    let sdk = QuicknodeSdk::new(&SdkFullConfig::from_api_key(key))?;
    let result = crate::retry::retrying(global.retries, || sdk.admin.list_chains()).await;
    let ok = result.is_ok();
    print_status(&global, source, &redacted, Some(ok));
    result.map(|_| ()).map_err(Into::into)
}

/// Resolves the key just like the rest of the CLI — no prompting. Returns the
/// raw key (callers must redact before printing) along with its source.
fn resolve_non_interactive(global: &GlobalArgs) -> Result<(String, KeySource), CliError> {
    let path = global.resolve_config_path();
    config::resolve_api_key(global.api_key.as_deref(), path.as_deref(), false, || {
        unreachable!("prompt disabled for auth status/whoami")
    })
}

/// Show the last 4 chars only. Char-based slicing — never panics on multi-byte input.
fn redact(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        "****".to_string()
    } else {
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("****{tail}")
    }
}

fn print_status(global: &GlobalArgs, source: KeySource, redacted: &str, validated: Option<bool>) {
    let v = serde_json::json!({
        "source": source.label(),
        "key": redacted,
        "validated": validated,
    });
    let stdout_is_tty = std::io::stdout().is_terminal();
    match global.resolve_format(stdout_is_tty) {
        crate::output::Format::Json => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        crate::output::Format::Yaml => {
            print!("{}", serde_yml::to_string(&v).unwrap_or_default());
        }
        crate::output::Format::Toon => {
            println!("{}", toon_format::encode_default(&v).unwrap_or_default());
        }
        crate::output::Format::Table | crate::output::Format::Md => {
            println!("source : {}", source.label());
            println!("key    : {}", redacted);
            if let Some(ok) = validated {
                println!("status : {}", if ok { "valid" } else { "rejected by API" });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redact_short_keys_returns_just_stars() {
        assert_eq!(redact(""), "****");
        assert_eq!(redact("abcd"), "****");
    }

    #[test]
    fn redact_ascii_shows_last_four() {
        assert_eq!(redact("abcdefgh"), "****efgh");
    }

    #[test]
    fn redact_multibyte_does_not_panic() {
        // αβγδεζη — each char is 2 bytes in UTF-8. Byte-slicing the last 4
        // bytes would land in the middle of a char and panic.
        let out = redact("αβγδεζη");
        assert_eq!(out.chars().count(), 8); // "****" + last 4 chars
        assert!(out.ends_with("δεζη"));
    }
}
