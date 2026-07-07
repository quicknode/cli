//! `qn auth {login,logout,whoami,status}` — manage the CLI's stored API key.
//!
//! Login: prompts for the key (hidden input), writes `~/.config/qn/config.toml`.
//! Logout: deletes that file.
//! Whoami: shows where the resolved key came from and the account identity
//! (id, name, plan), and confirms it works by calling the account-info API.
//! Status: same as whoami minus the network round-trip (no account details).

use std::io::{IsTerminal, Write};

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::QuicknodeSdk;

use crate::config::{self, KeySource};
use crate::context::{sdk_config_with_base, GlobalArgs};
use crate::errors::CliError;
use crate::output::osc8_link;

/// The signup URL shown to the user in the login welcome. Stays clean.
const SIGNUP_URL: &str = "https://www.quicknode.com/signup";

/// The click target for the signup link: the clean URL plus CLI attribution
/// params. Carried via an OSC 8 hyperlink so it never appears in the visible
/// text. Must keep [`SIGNUP_URL`] as its prefix (asserted in tests).
const SIGNUP_URL_TAGGED: &str =
    "https://www.quicknode.com/signup?utm_source=cli&utm_medium=cli&utm_campaign=clams";

/// Whether to emit an OSC 8 hyperlink for the signup line. Mirrors the
/// color-suppression rules in [`crate::output::OutputCtx::detect_with`]: a
/// hyperlink is an ANSI escape, so the same opt-outs apply. The welcome only
/// prints on the interactive TTY path, so no TTY check is needed here. Taking
/// the flag + env values as arguments keeps this testable without mutating
/// process env (which races across parallel tests).
fn hyperlinks_enabled(
    no_color: bool,
    no_color_env: Option<std::ffi::OsString>,
    term_env: Option<String>,
) -> bool {
    !no_color
        && no_color_env.map_or(true, |v| v.is_empty())
        && term_env.map_or(true, |t| t != "dumb")
}

/// The signup line for the login welcome: an OSC 8 hyperlink (clean visible
/// text, tagged target) when `hyperlinks` is true, otherwise the plain clean
/// URL with no params.
fn signup_link(hyperlinks: bool) -> String {
    if hyperlinks {
        osc8_link(SIGNUP_URL_TAGGED, SIGNUP_URL)
    } else {
        SIGNUP_URL.to_string()
    }
}

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
        CliError::Arg(
            "no config directory available: set HOME (or USERPROFILE on Windows), or pass --config-file <PATH>"
                .to_string(),
        )
    })?;

    let key = match args.api_key.or(global.api_key) {
        Some(k) => k.trim().to_string(),
        None => {
            if !config::can_prompt() || global.no_input {
                return Err(CliError::Arg(
                    "no TTY available; pass --api-key to log in non-interactively".to_string(),
                ));
            }
            if !global.quiet {
                let signup = signup_link(hyperlinks_enabled(
                    global.no_color,
                    std::env::var_os("NO_COLOR"),
                    std::env::var("TERM").ok(),
                ));
                let _ = writeln!(
                    std::io::stderr(),
                    "Welcome! The qn CLI uses a Quicknode API key to manage your account.\n\
                     Your key is stored locally in {}.\n\n  \
                     Get an API key:  https://dashboard.quicknode.com/api-keys\n  \
                     Need an account? {}\n",
                    path.display(),
                    signup
                );
            }
            config::prompt_for_api_key()?
        }
    };

    if key.is_empty() {
        return Err(CliError::Arg("API key cannot be empty".to_string()));
    }

    // Quick validation against the API so we don't silently save a bogus key.
    let sdk = QuicknodeSdk::new(&sdk_config_with_base(
        key.clone(),
        global.base_url.as_deref(),
    )?)?;
    crate::retry::retrying(global.retries, || sdk.admin.account_info()).await?;

    config::save_api_key(&path, &key)?;
    if !global.quiet {
        let _ = writeln!(std::io::stderr(), "✓ Saved API key to {}", path.display());
        let _ = writeln!(
            std::io::stderr(),
            "Tip: run 'qn agent context' for a machine-readable usage guide."
        );
    }
    Ok(())
}

fn logout(global: GlobalArgs) -> Result<(), CliError> {
    let path = global.resolve_config_path().ok_or_else(|| {
        CliError::Arg(
            "no config directory available: set HOME (or USERPROFILE on Windows), or pass --config-file <PATH>"
                .to_string(),
        )
    })?;
    config::clear_api_key(&path)?;
    if !global.quiet {
        let _ = writeln!(std::io::stderr(), "✓ Removed saved API key");
    }
    Ok(())
}

fn status(global: GlobalArgs) -> Result<(), CliError> {
    let (key, source) = resolve_non_interactive(&global)?;
    print_status(&global, source, &redact(&key), None, None);
    Ok(())
}

async fn whoami(global: GlobalArgs) -> Result<(), CliError> {
    let (key, source) = resolve_non_interactive(&global)?;
    let redacted = redact(&key);
    let sdk = QuicknodeSdk::new(&sdk_config_with_base(key, global.base_url.as_deref())?)?;
    // account_info doubles as the liveness probe: one call validates the key
    // and returns the account identity we display below.
    let result = crate::retry::retrying(global.retries, || sdk.admin.account_info()).await;
    let account = result.as_ref().ok().and_then(|r| r.data.clone());
    print_status(
        &global,
        source,
        &redacted,
        Some(result.is_ok()),
        account.as_ref(),
    );
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

/// Renders a subscription as a compact `plan_name (status, interval)` string,
/// skipping any absent part. Returns `None` when there is nothing to show.
fn plan_summary(sub: &quicknode_sdk::admin::AccountSubscription) -> Option<String> {
    let name = sub.plan_name.as_deref();
    let mut quals = Vec::new();
    if let Some(s) = sub.status.as_deref() {
        quals.push(s);
    }
    if let Some(i) = sub.interval.as_deref() {
        quals.push(i);
    }
    match (name, quals.is_empty()) {
        (None, true) => None,
        (Some(n), true) => Some(n.to_string()),
        (None, false) => Some(quals.join(", ")),
        (Some(n), false) => Some(format!("{n} ({})", quals.join(", "))),
    }
}

fn print_status(
    global: &GlobalArgs,
    source: KeySource,
    redacted: &str,
    validated: Option<bool>,
    account: Option<&quicknode_sdk::admin::AccountInfo>,
) {
    let sub = account.and_then(|a| a.subscription.as_ref());
    let plan = sub.and_then(plan_summary);
    let v = serde_json::json!({
        "source": source.label(),
        "key": redacted,
        "validated": validated,
        "account_id": account.map(|a| a.id),
        "account_name": account.map(|a| a.name.clone()),
        "plan": plan,
        "plan_status": sub.and_then(|s| s.status.clone()),
        "plan_interval": sub.and_then(|s| s.interval.clone()),
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
            if let Some(a) = account {
                println!("account: {} ({})", a.id, a.name);
                println!("plan   : {}", plan.as_deref().unwrap_or("<none>"));
            }
            if let Some(ok) = validated {
                println!("status : {}", if ok { "valid" } else { "rejected by API" });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hyperlinks_enabled, redact, signup_link, SIGNUP_URL, SIGNUP_URL_TAGGED};

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

    #[test]
    fn tagged_signup_url_extends_clean_url_with_utm_params() {
        // The visible label and the click target must not drift apart: the
        // tagged target is the clean URL plus the CLI attribution params.
        assert!(
            SIGNUP_URL_TAGGED.starts_with(SIGNUP_URL),
            "tagged URL must keep the clean URL as its prefix"
        );
        assert!(SIGNUP_URL_TAGGED.contains("utm_source=cli"));
        assert!(SIGNUP_URL_TAGGED.contains("utm_medium=cli"));
        assert!(SIGNUP_URL_TAGGED.contains("utm_campaign=clams"));
        // The clean URL carries no params — nothing is shown to the user.
        assert!(!SIGNUP_URL.contains('?'));
    }

    #[test]
    fn signup_link_hyperlink_hides_params_in_visible_text() {
        let link = signup_link(true);
        // Visible label is the clean URL; the params live in the escape target.
        assert!(link.contains(SIGNUP_URL_TAGGED), "target missing");
        assert!(link.contains('\x1b'), "expected OSC 8 escape");
    }

    #[test]
    fn signup_link_plain_is_clean_url_without_params() {
        let link = signup_link(false);
        assert_eq!(link, SIGNUP_URL);
        assert!(!link.contains('\x1b'));
        assert!(!link.contains("utm_"));
    }

    #[test]
    fn hyperlinks_disabled_by_no_color_flag() {
        assert!(!hyperlinks_enabled(true, None, None));
    }

    #[test]
    fn hyperlinks_disabled_by_no_color_env() {
        assert!(!hyperlinks_enabled(false, Some("1".into()), None));
    }

    #[test]
    fn empty_no_color_env_does_not_disable_hyperlinks() {
        assert!(hyperlinks_enabled(false, Some("".into()), None));
    }

    #[test]
    fn hyperlinks_disabled_by_term_dumb() {
        assert!(!hyperlinks_enabled(false, None, Some("dumb".into())));
    }

    #[test]
    fn hyperlinks_enabled_with_no_overrides() {
        assert!(hyperlinks_enabled(
            false,
            None,
            Some("xterm-256color".into())
        ));
    }
}
