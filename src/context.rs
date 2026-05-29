//! Runtime context shared by every command.
//!
//! Builds the `QuicknodeSdk` from `GlobalArgs`, attaches an `OutputCtx`, and
//! records where the API key came from so `qn auth whoami` can report it.

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
    /// (then default `Table`) when we build the [`Ctx`].
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
    /// Resolve the output format: CLI flag > config file > `Format::Table`.
    /// Used by [`Ctx::from_global`] and `auth` (which doesn't build a Ctx).
    pub fn resolve_format(&self) -> Format {
        self.resolve_output().0
    }

    /// Resolve `(format, wide)` together so we only read the config file once.
    ///
    /// For each: CLI flag > config file > built-in default. `--wide` is purely
    /// additive — the flag sets it true; the config file can also set it true;
    /// otherwise it's false.
    pub fn resolve_output(&self) -> (Format, bool) {
        let mut format = self.format;
        let mut wide = self.wide;
        if format.is_none() || !wide {
            if let Some(p) = config::config_path() {
                if let Ok(Some(cfg)) = config::load_from(&p) {
                    if format.is_none() {
                        format = cfg.output.format;
                    }
                    if !wide {
                        wide = cfg.output.wide;
                    }
                }
            }
        }
        (format.unwrap_or_default(), wide)
    }
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
        let (format, wide) = global.resolve_output();

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
        let out = OutputCtx::detect(format, global.no_color, global.quiet, global.verbose, wide);

        Ok(Self {
            sdk,
            out,
            global,
            key_source,
            config_path,
        })
    }
}
