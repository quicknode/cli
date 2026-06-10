//! Runtime context shared by every command.
//!
//! Builds the `QuicknodeSdk` from `GlobalArgs` and attaches an `OutputCtx`.

use std::io::IsTerminal;

use quicknode_sdk::{
    AdminConfig, KvStoreConfig, QuicknodeSdk, SdkFullConfig, StreamsConfig, WebhooksConfig,
};

use crate::config;
use crate::errors::CliError;
use crate::output::{Format, OutputCtx};

/// Top-level flags inherited by every subcommand.
#[derive(Debug, Clone, Default)]
pub struct GlobalArgs {
    pub api_key: Option<String>,
    /// `--config-file`: alternate config TOML. `None` means the default path
    /// (`~/.config/qn/config.toml`).
    pub config_file: Option<std::path::PathBuf>,
    /// `None` means the user didn't pass `--format`; resolve via config file
    /// (then the TTY-aware default: `Table` on a TTY, `Toon` off) when we
    /// build the [`Ctx`].
    pub format: Option<Format>,
    pub wide: bool,
    pub no_color: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub no_input: bool,
    pub yes_count: u8,
    /// Max automatic retries for read-only API calls (see `crate::retry`).
    /// `Default` yields 0 (no retries) — the CLI default of 3 comes from clap.
    pub retries: u32,
    pub base_url: Option<String>,
}

impl GlobalArgs {
    /// Resolve the output format: CLI flag > config file > TTY-aware default
    /// (`Table` on a TTY, `Toon` off).
    /// Used by [`Ctx::from_global`] and `auth` (which doesn't build a Ctx).
    pub fn resolve_format(&self, stdout_is_tty: bool) -> Format {
        self.resolve_output(stdout_is_tty).0
    }

    /// Resolve `(format, wide)` together so we only read the config file once.
    ///
    /// For each: CLI flag > config file > built-in default. The format default
    /// is TTY-aware: `Table` when stdout is a terminal, `Toon` otherwise (so
    /// agents / piped callers get a structured format by default). `--wide` is
    /// purely additive — the flag sets it true; the config file can also set
    /// it true; otherwise it's false.
    pub fn resolve_output(&self, stdout_is_tty: bool) -> (Format, bool) {
        let (cfg_format, cfg_wide) = self.load_output_config();
        resolve_output_inner(self.format, self.wide, cfg_format, cfg_wide, stdout_is_tty)
    }

    /// The config file to read: `--config-file` if given, else the default path.
    pub fn resolve_config_path(&self) -> Option<std::path::PathBuf> {
        self.config_file.clone().or_else(config::config_path)
    }

    fn load_output_config(&self) -> (Option<Format>, bool) {
        let Some(p) = self.resolve_config_path() else {
            return (None, false);
        };
        match config::load_from(&p) {
            Ok(Some(cfg)) => (cfg.output.format, cfg.output.wide),
            _ => (None, false),
        }
    }
}

/// Pure form of [`GlobalArgs::resolve_output`] — separated so it can be
/// exhaustively unit-tested without touching the real config file. CLI values
/// win; otherwise we fall back to config; otherwise the TTY-aware default.
fn resolve_output_inner(
    flag_format: Option<Format>,
    flag_wide: bool,
    cfg_format: Option<Format>,
    cfg_wide: bool,
    stdout_is_tty: bool,
) -> (Format, bool) {
    let format = flag_format.or(cfg_format).unwrap_or(if stdout_is_tty {
        Format::Table
    } else {
        Format::Toon
    });
    let wide = flag_wide || cfg_wide;
    (format, wide)
}

pub struct Ctx {
    pub sdk: QuicknodeSdk,
    pub out: OutputCtx,
    pub global: GlobalArgs,
}

impl Ctx {
    /// Construct the SDK + output ctx from `global`. Resolves the API key per
    /// the documented precedence: flag > config file (`--config-file` path if
    /// given, else the default). If neither supplies a key we return
    /// `CliError::NoApiKey` — regular commands do not prompt; the user is
    /// directed to `qn auth login`.
    pub fn from_global(global: GlobalArgs) -> Result<Self, CliError> {
        let config_path = global.resolve_config_path();
        let stdout_is_tty = std::io::stdout().is_terminal();
        let (format, wide) = global.resolve_output(stdout_is_tty);

        let (api_key, _) = config::resolve_api_key(
            global.api_key.as_deref(),
            config_path.as_deref(),
            false,
            || unreachable!("prompt disabled for non-auth commands"),
        )?;

        let mut full = SdkFullConfig::from_api_key(api_key);

        // --base-url applies to every sub-client. Useful for wiremock tests and
        // on-prem mirrors. Each sub-client has its own base path under the host
        // so we suffix correctly.
        if let Some(base) = &global.base_url {
            let trimmed = validate_base_url(base)?;
            let trimmed = trimmed.as_str();
            full.admin = Some(AdminConfig {
                base_url: Some(format!("{trimmed}/v0/")),
            });
            full.streams = Some(StreamsConfig {
                base_url: Some(format!("{trimmed}/streams/rest/v1/")),
            });
            full.webhooks = Some(WebhooksConfig {
                base_url: Some(format!("{trimmed}/webhooks/rest/v1/")),
            });
            full.kvstore = Some(KvStoreConfig {
                base_url: Some(format!("{trimmed}/kv/rest/v1/")),
            });
        }

        let sdk = QuicknodeSdk::new(&full)?;
        let out = OutputCtx::detect_with(
            format,
            global.no_color,
            global.quiet,
            global.verbose,
            wide,
            stdout_is_tty,
            std::env::var_os("NO_COLOR"),
            std::env::var("TERM").ok(),
        );

        Ok(Self { sdk, out, global })
    }
}

/// Validates a user-supplied `--base-url` and returns it with any trailing
/// slash stripped. Rejects non-http(s) schemes, embedded userinfo, query/
/// fragment, and non-root paths so we can't accidentally splice attacker-
/// controlled segments into the SDK's hard-coded sub-client paths.
fn validate_base_url(base: &str) -> Result<String, CliError> {
    let parsed = url::Url::parse(base)
        .map_err(|_| CliError::Arg(format!("--base-url '{base}' is not a valid URL")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CliError::Arg(format!(
                "--base-url scheme '{other}' is not allowed; use http or https"
            )))
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CliError::Arg(
            "--base-url must not contain userinfo (username/password)".into(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(CliError::Arg(
            "--base-url must not contain a query string or fragment".into(),
        ));
    }
    if !matches!(parsed.path(), "" | "/") {
        return Err(CliError::Arg("--base-url must not contain a path".into()));
    }
    Ok(base.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_format_wins_over_config_and_tty_default() {
        let (f, _) =
            resolve_output_inner(Some(Format::Json), false, Some(Format::Yaml), false, true);
        assert_eq!(f, Format::Json);
        let (f, _) =
            resolve_output_inner(Some(Format::Json), false, Some(Format::Yaml), false, false);
        assert_eq!(f, Format::Json);
    }

    #[test]
    fn config_format_wins_over_tty_default() {
        let (f, _) = resolve_output_inner(None, false, Some(Format::Yaml), false, true);
        assert_eq!(f, Format::Yaml);
        let (f, _) = resolve_output_inner(None, false, Some(Format::Yaml), false, false);
        assert_eq!(f, Format::Yaml);
    }

    #[test]
    fn default_is_table_when_stdout_is_a_tty() {
        let (f, _) = resolve_output_inner(None, false, None, false, true);
        assert_eq!(f, Format::Table);
    }

    #[test]
    fn default_is_toon_when_stdout_is_not_a_tty() {
        let (f, _) = resolve_output_inner(None, false, None, false, false);
        assert_eq!(f, Format::Toon);
    }

    #[test]
    fn wide_is_additive_between_flag_and_config() {
        // Flag alone.
        let (_, w) = resolve_output_inner(None, true, None, false, true);
        assert!(w);
        // Config alone.
        let (_, w) = resolve_output_inner(None, false, None, true, true);
        assert!(w);
        // Both.
        let (_, w) = resolve_output_inner(None, true, None, true, true);
        assert!(w);
        // Neither.
        let (_, w) = resolve_output_inner(None, false, None, false, true);
        assert!(!w);
    }

    #[test]
    fn base_url_accepts_plain_http_and_https() {
        assert_eq!(
            validate_base_url("https://api.quicknode.com").unwrap(),
            "https://api.quicknode.com"
        );
        assert_eq!(
            validate_base_url("http://127.0.0.1:8080/").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn base_url_rejects_non_http_schemes() {
        for bad in ["file:///etc/passwd", "ftp://x", "javascript:alert(1)"] {
            assert!(validate_base_url(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn base_url_rejects_userinfo() {
        assert!(validate_base_url("https://user:pass@evil/").is_err());
        assert!(validate_base_url("https://user@evil/").is_err());
    }

    #[test]
    fn base_url_rejects_path_query_fragment() {
        assert!(validate_base_url("https://x/extra/path").is_err());
        assert!(validate_base_url("https://x/?q=1").is_err());
        assert!(validate_base_url("https://x/#frag").is_err());
    }

    #[test]
    fn base_url_rejects_garbage() {
        assert!(validate_base_url("not a url").is_err());
        assert!(validate_base_url("").is_err());
    }
}
