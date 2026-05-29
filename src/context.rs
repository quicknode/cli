//! Runtime context shared by every command.
//!
//! Builds the `QuicknodeSdk` from `GlobalArgs`, attaches an `OutputCtx`, and
//! records where the API key came from so `qn auth whoami` can report it.

use std::io::IsTerminal;
use std::path::PathBuf;

use quicknode_sdk::{
    AdminConfig, KvStoreConfig, QuicknodeSdk, SdkFullConfig, StreamsConfig, WebhooksConfig,
};

use crate::config::{self, KeySource};
use crate::errors::CliError;
use crate::output::{Format, OutputCtx};

/// Top-level flags inherited by every subcommand.
#[derive(Debug, Clone, Default)]
pub struct GlobalArgs {
    pub api_key: Option<String>,
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

    fn load_output_config(&self) -> (Option<Format>, bool) {
        let Some(p) = config::config_path() else {
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
    pub key_source: KeySource,
    pub config_path: Option<PathBuf>,
}

impl Ctx {
    /// Construct the SDK + output ctx from `global`. Resolves the API key per
    /// the documented precedence: flag > env > config file. If none of those
    /// supply a key we return `CliError::NoApiKey` — regular commands do not
    /// prompt; the user is directed to `qn auth login`.
    pub fn from_global(global: GlobalArgs) -> Result<Self, CliError> {
        let config_path = config::config_path();
        let env_key = config::read_env_api_key();
        let stdout_is_tty = std::io::stdout().is_terminal();
        let (format, wide) = global.resolve_output(stdout_is_tty);

        let (api_key, key_source) = config::resolve_api_key(
            global.api_key.as_deref(),
            env_key.as_deref(),
            config_path.as_deref(),
            false,
            || unreachable!("prompt disabled for non-auth commands"),
        )?;

        let mut full = SdkFullConfig::from_api_key(api_key);

        // --base-url applies to every sub-client. Useful for wiremock tests and
        // on-prem mirrors. Each sub-client has its own base path under the host
        // so we suffix correctly.
        if let Some(base) = &global.base_url {
            let trimmed = base.trim_end_matches('/');
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

        Ok(Self {
            sdk,
            out,
            global,
            key_source,
            config_path,
        })
    }
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
}
