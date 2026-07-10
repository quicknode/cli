//! Top-level clap derive entry point.
//!
//! This file is the single source of truth for the CLI shape. Subcommand
//! bodies live under `commands::*` and dispatch happens via [`Cli::run`].

use std::io::Write;

use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use crate::commands;
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::Format;

/// qn — command-line interface for the Quicknode API.
#[derive(Debug, Parser)]
#[command(
    name = "qn",
    version,
    about = "Command-line interface for the Quicknode API.",
    long_about = "qn lets you manage Quicknode endpoints, streams, webhooks, and the KV store from the terminal.\n\n\
                  Use `qn <noun> --help` (e.g. `qn endpoint --help`) for command details.\n\n\
                  Authentication is resolved in this order: --api-key flag, then the config file\n\
                  (--config-file path if given, else ~/.config/qn/config.toml). Run `qn auth login`\n\
                  to save a key the first time.",
    propagate_version = true,
    disable_help_subcommand = true,
    // The auto-generated -h/-V land under a separate "Options" heading; we
    // re-declare them below so they group with the other global flags.
    disable_help_flag = true,
    disable_version_flag = true,
    after_help = "Examples:\n  \
        qn auth login\n  \
        qn endpoint create --chain ethereum --network mainnet\n  \
        qn endpoint list -o json\n  \
        qn endpoint logs ep-1234 --from 1h\n  \
        qn chain list\n\n\
        AI agents: run 'qn agent context' for a machine-readable usage guide.",
    // Group the global flags under their own heading in every subcommand's
    // --help, so command-specific flags surface first under "Options".
    next_help_heading = "Global options"
)]
pub struct Cli {
    /// API key. Overrides the config file.
    #[arg(long, global = true)]
    pub api_key: Option<String>,

    /// Path to an alternate config file (default: ~/.config/qn/config.toml).
    #[arg(long, global = true, value_name = "PATH")]
    pub config_file: Option<std::path::PathBuf>,

    /// Output format. `table` is the human view; the others are
    /// pipeline-friendly serialized forms. If unset, falls back to the
    /// `[output] format = "…"` value in ~/.config/qn/config.toml, then the
    /// TTY-aware default: `table` when stdout is a terminal, `json` otherwise.
    #[arg(short = 'o', long = "format", global = true, value_enum)]
    pub format: Option<Format>,

    /// Disable ANSI colors. Also honored: NO_COLOR env var, TERM=dumb, non-TTY stdout.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Suppress non-essential output (state-change confirmations on stderr).
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Show additional columns in list-style tables (e.g. URLs in `endpoint list`).
    /// Mirrors `kubectl get -o wide`. Only affects `table` and `md` formats —
    /// `json`/`yaml`/`toon` always include everything.
    #[arg(short = 'w', long = "wide", global = true)]
    pub wide: bool,

    /// Verbose output: include error bodies and other details.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Never prompt interactively; fail with a clear message if input is needed.
    #[arg(long, global = true)]
    pub no_input: bool,

    /// Max automatic retries for read-only commands on transient failures
    /// (HTTP 429/500/502/503/504, timeouts). Uses exponential backoff with
    /// jitter. 0 disables retries. Commands that modify resources never retry.
    #[arg(long, global = true, default_value_t = 3, value_name = "N")]
    pub retries: u32,

    /// Skip confirmation prompts on destructive operations.
    #[arg(short = 'y', long = "yes", global = true, action = ArgAction::Count)]
    pub yes: u8,

    /// Override the Quicknode API base URL (used for testing or on-prem mirrors).
    /// All four sub-clients (admin/streams/webhooks/kv) hang off this host.
    #[arg(long, global = true, hide = true)]
    pub base_url: Option<String>,

    /// Path prefix inserted between the host and each sub-client's path (e.g.
    /// `/console-api`). For reverse-proxy / gateway environments. Requires
    /// `--base-url`.
    #[arg(long, global = true, hide = true)]
    pub base_prefix: Option<String>,

    /// Print help (see a summary with '-h').
    #[arg(short = 'h', long, global = true, action = ArgAction::Help)]
    pub help: Option<bool>,

    /// Print version.
    #[arg(short = 'V', long, global = true, action = ArgAction::Version)]
    pub version: Option<bool>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage CLI authentication (API key).
    Auth(commands::auth::Args),

    /// Resources for AI agents and automated tools.
    Agent(commands::agent::Args),

    /// Manage RPC endpoints on your account.
    #[command(visible_alias = "endpoints")]
    Endpoint(commands::endpoint::Args),

    /// Manage teams.
    #[command(visible_alias = "teams")]
    Team(commands::team::Args),

    /// View account usage.
    Usage(commands::usage::Args),

    /// View account or endpoint metrics.
    Metrics(commands::metrics::Args),

    /// List supported blockchains.
    #[command(visible_alias = "chains")]
    Chain(commands::chain::Args),

    /// View invoices and payments.
    Billing(commands::billing::Args),

    /// Manage blockchain data streams.
    #[command(visible_alias = "streams")]
    Stream(commands::stream::Args),

    /// Manage filter-template webhooks.
    #[command(visible_alias = "webhooks")]
    Webhook(commands::webhook::Args),

    /// Manage the Quicknode KV store (sets and lists).
    Kv(commands::kv::Args),

    /// Run SQL queries and inspect cluster schemas.
    Sql(commands::sql::Args),

    /// Make JSON-RPC calls against your Tooling Access endpoint.
    Rpc(commands::rpc::Args),

    /// Manage Tooling Access (the endpoint `qn rpc` uses).
    #[command(name = "tooling-access")]
    ToolingAccess(commands::tooling_access::Args),

    /// Generate shell completion scripts.
    ///
    /// When installing qn through a package manager, it's possible that no
    /// additional shell configuration is necessary to gain completion support.
    /// Homebrew and distro packages place the script for you.
    ///
    /// If you need to set up completions manually, follow the instructions
    /// below. The exact config file locations might vary based on your system.
    /// Make sure to restart your shell before testing whether completions are
    /// working.
    #[command(after_long_help = "### bash\n\n  \
        First, ensure that you install `bash-completion` using your package manager.\n\n  \
        After, add this to your `~/.bashrc`:\n\n      \
        eval \"$(qn completions bash)\"\n\n\
        ### zsh\n\n  \
        Homebrew already creates this `_qn` file for you on `brew install`. To\n  \
        set it up manually, generate the script into a directory on your\n  \
        `$fpath` (Apple Silicon shown; Intel brew uses\n  \
        `/usr/local/share/zsh/site-functions`):\n\n      \
        qn completions zsh > /opt/homebrew/share/zsh/site-functions/_qn\n\n  \
        Ensure that the following is present in your `~/.zshrc`:\n\n      \
        autoload -U compinit\n      \
        compinit\n\n  \
        See the zsh manual for details:\n  \
        https://zsh.sourceforge.io/Doc/Release/Completion-System.html\n\n\
        ### fish\n\n  \
        Generate a `qn.fish` completion script:\n\n      \
        qn completions fish > ~/.config/fish/completions/qn.fish\n\n\
        ### PowerShell\n\n  \
        Add the following line to your profile script (`$PROFILE`):\n\n      \
        qn completions powershell | Out-String | Invoke-Expression\n\n  \
        Or append the generated script so it loads each session:\n\n      \
        qn completions powershell >> $PROFILE")]
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },
}

impl Cli {
    /// Build a [`GlobalArgs`] suitable for [`Ctx::from_global`].
    pub fn global_args(&self) -> GlobalArgs {
        GlobalArgs {
            api_key: self.api_key.clone(),
            config_file: self.config_file.clone(),
            format: self.format,
            wide: self.wide,
            // format resolved-from-config in Ctx::from_global; auth.rs falls
            // back to Table directly if it stays None there.
            no_color: self.no_color,
            quiet: self.quiet,
            verbose: self.verbose,
            no_input: self.no_input,
            yes_count: self.yes,
            retries: self.retries,
            base_url: self.base_url.clone(),
            base_prefix: self.base_prefix.clone(),
        }
    }

    /// Dispatch the parsed command.
    ///
    /// Some commands (auth, completions) are handled without constructing the
    /// SDK — they have nothing to talk to and shouldn't trigger an API-key
    /// prompt.
    pub async fn run(self) -> Result<(), CliError> {
        let global = self.global_args();
        match self.command {
            Command::Completions { shell } => {
                let mut cmd = <Self as CommandFactory>::command();
                let bin_name = cmd.get_name().to_string();
                let mut out = std::io::stdout().lock();
                clap_complete::generate(shell, &mut cmd, bin_name, &mut out);
                out.flush()?;
                Ok(())
            }
            Command::Auth(args) => commands::auth::run(args, global).await,
            Command::Agent(args) => commands::agent::run(args, global).await,
            Command::Endpoint(args) => {
                commands::endpoint::run(args, Ctx::from_global(global)?).await
            }
            Command::Team(args) => commands::team::run(args, Ctx::from_global(global)?).await,
            Command::Usage(args) => commands::usage::run(args, Ctx::from_global(global)?).await,
            Command::Metrics(args) => commands::metrics::run(args, Ctx::from_global(global)?).await,
            Command::Chain(args) => commands::chain::run(args, Ctx::from_global(global)?).await,
            Command::Billing(args) => commands::billing::run(args, Ctx::from_global(global)?).await,
            Command::Stream(args) => commands::stream::run(args, Ctx::from_global(global)?).await,
            Command::Webhook(args) => commands::webhook::run(args, Ctx::from_global(global)?).await,
            Command::Kv(args) => commands::kv::run(args, Ctx::from_global(global)?).await,
            Command::Sql(args) => commands::sql::run(args, Ctx::from_global(global)?).await,
            // rpc builds its own Ctx (it seeds the SDK from the on-disk token
            // cache before construction), so it takes `global` directly.
            Command::Rpc(args) => commands::rpc::run(args, global).await,
            Command::ToolingAccess(args) => {
                commands::tooling_access::run(args, Ctx::from_global(global)?).await
            }
        }
    }
}
