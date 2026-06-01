//! `qn webhook …` — filter-template webhooks.

mod actions;
mod render;

use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use quicknode_sdk::webhooks::WebhookStartFrom;

use crate::context::Ctx;
use crate::errors::CliError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: WebhookCmd,
}

#[derive(Debug, Subcommand)]
pub enum WebhookCmd {
    /// List webhooks on the account.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Show a single webhook.
    Show { id: String },
    /// Create a webhook from a filter template.
    Create(Box<CreateArgs>),
    /// Update name/email/destination on a webhook (without changing the template).
    Update(UpdateArgs),
    /// Update the template arguments on a webhook (and optionally other fields).
    UpdateTemplate(Box<UpdateTemplateArgs>),
    /// Delete a webhook.
    Delete { id: String },
    /// Delete every webhook on the account.
    DeleteAll,
    /// Activate a webhook (resume delivery).
    Activate(ActivateArgs),
    /// Pause a webhook.
    Pause { id: String },
    /// Count of currently enabled webhooks.
    EnabledCount,
}

#[derive(Debug, ClapArgs)]
pub struct ListArgs {
    #[arg(long)]
    pub limit: Option<i64>,
    #[arg(long)]
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TemplateKind {
    EvmWallet,
    EvmContractEvents,
    EvmAbi,
    SolanaWallet,
    BitcoinWallet,
    XrplWallet,
    HyperliquidWalletEvents,
    StellarWalletTransactions,
}

#[derive(Debug, ClapArgs)]
pub struct CreateArgs {
    /// Webhook name.
    #[arg(long)]
    pub name: String,
    /// Network (e.g. `ethereum-mainnet`).
    #[arg(long)]
    pub network: String,
    /// Destination URL.
    #[arg(long)]
    pub url: String,
    /// Optional security token (server generates one if omitted).
    #[arg(long)]
    pub security_token: Option<String>,
    /// Payload compression (`gzip` or `none`).
    #[arg(long)]
    pub compression: Option<String>,
    /// Optional notification email.
    #[arg(long)]
    pub notification_email: Option<String>,

    /// Filter template.
    #[arg(long, value_enum)]
    pub template: TemplateKind,

    /// For wallet-style templates: wallet addresses (repeat).
    #[arg(long = "wallet")]
    pub wallets: Vec<String>,
    /// For Solana wallet template: account addresses (repeat).
    #[arg(long = "account")]
    pub accounts: Vec<String>,
    /// For contract-events and abi templates: contract addresses (repeat).
    #[arg(long = "contract")]
    pub contracts: Vec<String>,
    /// For contract-events template: optional event topic hashes (repeat).
    #[arg(long = "event-hash")]
    pub event_hashes: Vec<String>,
    /// For abi template: contract ABI JSON inline.
    #[arg(long, conflicts_with = "abi_file")]
    pub abi: Option<String>,
    /// For abi template: path to a file with the contract ABI JSON.
    #[arg(long)]
    pub abi_file: Option<PathBuf>,
}

#[derive(Debug, ClapArgs)]
pub struct UpdateArgs {
    pub id: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub notification_email: Option<String>,
    /// New destination URL.
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub security_token: Option<String>,
    #[arg(long)]
    pub compression: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct UpdateTemplateArgs {
    pub id: String,
    /// New filter template (same flags as `create`).
    #[arg(long, value_enum)]
    pub template: TemplateKind,
    #[arg(long = "wallet")]
    pub wallets: Vec<String>,
    #[arg(long = "account")]
    pub accounts: Vec<String>,
    #[arg(long = "contract")]
    pub contracts: Vec<String>,
    #[arg(long = "event-hash")]
    pub event_hashes: Vec<String>,
    #[arg(long, conflicts_with = "abi_file")]
    pub abi: Option<String>,
    #[arg(long)]
    pub abi_file: Option<PathBuf>,

    /// Optionally also rename.
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub notification_email: Option<String>,
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub security_token: Option<String>,
    #[arg(long)]
    pub compression: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct ActivateArgs {
    pub id: String,
    /// Where to resume from.
    #[arg(long, value_enum, default_value = "latest")]
    pub start_from: StartFromArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StartFromArg {
    Last,
    Latest,
}

impl From<StartFromArg> for WebhookStartFrom {
    fn from(s: StartFromArg) -> Self {
        match s {
            StartFromArg::Last => WebhookStartFrom::Last,
            StartFromArg::Latest => WebhookStartFrom::Latest,
        }
    }
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        WebhookCmd::List(a) => actions::list(a, ctx).await,
        WebhookCmd::Show { id } => actions::show(&id, ctx).await,
        WebhookCmd::Create(a) => actions::create(*a, ctx).await,
        WebhookCmd::Update(a) => actions::update(a, ctx).await,
        WebhookCmd::UpdateTemplate(a) => actions::update_template(*a, ctx).await,
        WebhookCmd::Delete { id } => actions::delete(&id, ctx).await,
        WebhookCmd::DeleteAll => actions::delete_all(ctx).await,
        WebhookCmd::Activate(a) => actions::activate(a, ctx).await,
        WebhookCmd::Pause { id } => actions::pause(&id, ctx).await,
        WebhookCmd::EnabledCount => actions::enabled_count(ctx).await,
    }
}
