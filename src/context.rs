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
use crate::output::OutputCtx;

/// Top-level flags inherited by every subcommand.
#[derive(Debug, Clone, Default)]
pub struct GlobalArgs {
    pub api_key: Option<String>,
    pub json: bool,
    pub no_color: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub no_input: bool,
    pub yes_count: u8,
    pub base_url: Option<String>,
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
    /// the documented precedence.
    pub fn from_global(global: GlobalArgs) -> Result<Self, CliError> {
        let config_path = config::config_path();
        let env_key = config::read_env_api_key();
        let can_prompt = !global.no_input && config::can_prompt();

        let (api_key, key_source) = config::resolve_api_key(
            global.api_key.as_deref(),
            env_key.as_deref(),
            config_path.as_deref(),
            can_prompt,
            config::prompt_for_api_key,
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
        let out = OutputCtx::detect(global.json, global.no_color, global.quiet, global.verbose);

        Ok(Self {
            sdk,
            out,
            global,
            key_source,
            config_path,
        })
    }
}
