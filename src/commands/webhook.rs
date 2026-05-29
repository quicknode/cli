//! `qn webhook …` — filter-template webhooks.

use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::webhooks::{
    ActivateWebhookParams, BitcoinWalletFilterTemplate, CreateWebhookFromTemplateParams,
    EvmAbiFilterTemplate, EvmContractEventsTemplate, EvmWalletFilterTemplate, GetWebhooksParams,
    HyperliquidWalletEventsFilterTemplate, SolanaWalletFilterTemplate,
    StellarWalletTransactionsFilterTemplate, TemplateArgs, UpdateWebhookParams,
    UpdateWebhookTemplateParams, WebhookDestinationAttributes, WebhookStartFrom,
    XrplWalletFilterTemplate,
};
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_typed, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};

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
        WebhookCmd::List(a) => list(a, ctx).await,
        WebhookCmd::Show { id } => show(&id, ctx).await,
        WebhookCmd::Create(a) => create(*a, ctx).await,
        WebhookCmd::Update(a) => update(a, ctx).await,
        WebhookCmd::UpdateTemplate(a) => update_template(*a, ctx).await,
        WebhookCmd::Delete { id } => delete(&id, ctx).await,
        WebhookCmd::DeleteAll => delete_all(ctx).await,
        WebhookCmd::Activate(a) => activate(a, ctx).await,
        WebhookCmd::Pause { id } => pause(&id, ctx).await,
        WebhookCmd::EnabledCount => enabled_count(ctx).await,
    }
}

async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = GetWebhooksParams {
        limit: a.limit,
        offset: a.offset,
    };
    let resp = ctx.sdk.webhooks.list_webhooks(&params).await?;
    crate::output::emit(&ctx.out, &WebhooksListView(resp))
}

async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let w = ctx.sdk.webhooks.get_webhook(id).await?;
    crate::output::emit(&ctx.out, &WebhookView(w))
}

async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let template_args = build_template_args(
        a.template,
        a.wallets,
        a.accounts,
        a.contracts,
        a.event_hashes,
        a.abi,
        a.abi_file,
    )?;
    let params = CreateWebhookFromTemplateParams {
        name: a.name,
        network: a.network,
        notification_email: a.notification_email,
        destination_attributes: WebhookDestinationAttributes {
            url: a.url,
            security_token: a.security_token,
            compression: a.compression,
        },
        template_args,
    };
    let w = ctx
        .sdk
        .webhooks
        .create_webhook_from_template(&params)
        .await?;
    ctx.out.note(&format!("✓ Created webhook {}", w.id));
    crate::output::emit(&ctx.out, &WebhookView(w))
}

async fn update(a: UpdateArgs, ctx: Ctx) -> Result<(), CliError> {
    let destination = match a.url {
        Some(url) => Some(WebhookDestinationAttributes {
            url,
            security_token: a.security_token,
            compression: a.compression,
        }),
        None if a.security_token.is_some() || a.compression.is_some() => {
            return Err(CliError::Arg(
                "to change destination fields, also supply --url".into(),
            ));
        }
        None => None,
    };
    let params = UpdateWebhookParams {
        name: a.name,
        notification_email: a.notification_email,
        destination_attributes: destination,
    };
    let w = ctx.sdk.webhooks.update_webhook(&a.id, &params).await?;
    ctx.out.note(&format!("✓ Updated webhook {}", a.id));
    crate::output::emit(&ctx.out, &WebhookView(w))
}

async fn update_template(a: UpdateTemplateArgs, ctx: Ctx) -> Result<(), CliError> {
    let template_args = build_template_args(
        a.template,
        a.wallets,
        a.accounts,
        a.contracts,
        a.event_hashes,
        a.abi,
        a.abi_file,
    )?;
    let destination = a.url.map(|url| WebhookDestinationAttributes {
        url,
        security_token: a.security_token,
        compression: a.compression,
    });
    let params = UpdateWebhookTemplateParams {
        name: a.name,
        notification_email: a.notification_email,
        destination_attributes: destination,
        template_args,
    };
    let w = ctx
        .sdk
        .webhooks
        .update_webhook_template(&a.id, &params)
        .await?;
    ctx.out
        .note(&format!("✓ Updated template on webhook {}", a.id));
    crate::output::emit(&ctx.out, &WebhookView(w))
}

async fn delete(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete webhook {id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.webhooks.delete_webhook(id).await?;
    ctx.out.note(&format!("✓ Deleted webhook {id}"));
    Ok(())
}

async fn delete_all(ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Severe, cfg)? {
        true => true,
        false => prompt_typed(
            "Type 'delete-all' to delete EVERY webhook on the account",
            "delete-all",
        )?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.webhooks.delete_all_webhooks().await?;
    ctx.out.note("✓ Deleted all webhooks");
    Ok(())
}

async fn activate(a: ActivateArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = ActivateWebhookParams {
        start_from: a.start_from.into(),
    };
    ctx.sdk.webhooks.activate_webhook(&a.id, &params).await?;
    ctx.out.note(&format!("✓ Activated webhook {}", a.id));
    Ok(())
}

async fn pause(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.webhooks.pause_webhook(id).await?;
    ctx.out.note(&format!("✓ Paused webhook {id}"));
    Ok(())
}

async fn enabled_count(ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.webhooks.get_enabled_count().await?;
    if ctx.out.format.is_structured() {
        crate::output::emit(&ctx.out, &resp)
    } else {
        println!("{}", resp.total);
        Ok(())
    }
}

fn build_template_args(
    kind: TemplateKind,
    wallets: Vec<String>,
    accounts: Vec<String>,
    contracts: Vec<String>,
    event_hashes: Vec<String>,
    abi: Option<String>,
    abi_file: Option<PathBuf>,
) -> Result<TemplateArgs, CliError> {
    let event_hashes_opt = if event_hashes.is_empty() {
        None
    } else {
        Some(event_hashes)
    };
    match kind {
        TemplateKind::EvmWallet => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::EvmWalletFilter(EvmWalletFilterTemplate {
                wallets,
            }))
        }
        TemplateKind::SolanaWallet => {
            if accounts.is_empty() {
                return Err(CliError::Arg("supply at least one --account".into()));
            }
            Ok(TemplateArgs::SolanaWalletFilter(
                SolanaWalletFilterTemplate { accounts },
            ))
        }
        TemplateKind::BitcoinWallet => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::BitcoinWalletFilter(
                BitcoinWalletFilterTemplate { wallets },
            ))
        }
        TemplateKind::XrplWallet => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::XrplWalletFilter(XrplWalletFilterTemplate {
                wallets,
            }))
        }
        TemplateKind::HyperliquidWalletEvents => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::HyperliquidWalletEventsFilter(
                HyperliquidWalletEventsFilterTemplate { wallets },
            ))
        }
        TemplateKind::StellarWalletTransactions => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::StellarWalletTransactionsSourceAccountFilter(
                StellarWalletTransactionsFilterTemplate { wallets },
            ))
        }
        TemplateKind::EvmContractEvents => {
            if contracts.is_empty() {
                return Err(CliError::Arg("supply at least one --contract".into()));
            }
            Ok(TemplateArgs::EvmContractEvents(EvmContractEventsTemplate {
                contracts,
                event_hashes: event_hashes_opt,
            }))
        }
        TemplateKind::EvmAbi => {
            if contracts.is_empty() {
                return Err(CliError::Arg("supply at least one --contract".into()));
            }
            let abi_text = match (abi, abi_file) {
                (Some(s), None) => s,
                (None, Some(p)) => std::fs::read_to_string(&p)?,
                (None, None) => {
                    return Err(CliError::Arg("supply --abi or --abi-file".into()));
                }
                (Some(_), Some(_)) => {
                    return Err(CliError::Arg(
                        "supply only one of --abi or --abi-file".into(),
                    ));
                }
            };
            Ok(TemplateArgs::EvmAbiFilter(EvmAbiFilterTemplate {
                abi: abi_text,
                contracts,
            }))
        }
    }
}

fn require_wallets(wallets: &[String]) -> Result<(), CliError> {
    if wallets.is_empty() {
        Err(CliError::Arg("supply at least one --wallet".into()))
    } else {
        Ok(())
    }
}

// ----- renderers ----- //

#[derive(Serialize)]
struct WebhooksListView(quicknode_sdk::webhooks::ListWebhooksResponse);

impl Render for WebhooksListView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "NAME", "STATUS", "NETWORK", "TEMPLATE"],
        );
        for h in &self.0.data {
            t.add_row(vec![
                Cell::new(&h.id),
                Cell::new(&h.name),
                Cell::new(&h.status),
                Cell::new(&h.network),
                opt_cell(&h.template_id),
            ]);
        }
        write_table(w, &t)?;
        writeln!(
            w,
            "showing {}–{} of {}",
            self.0.page_info.offset + 1,
            (self.0.page_info.offset + self.0.data.len() as i64).min(self.0.page_info.total),
            self.0.page_info.total
        )
    }
}

#[derive(Serialize)]
struct WebhookView(quicknode_sdk::webhooks::Webhook);

impl Render for WebhookView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let h = &self.0;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(&h.id)]);
        t.add_row(vec![Cell::new("name"), Cell::new(&h.name)]);
        t.add_row(vec![Cell::new("status"), Cell::new(&h.status)]);
        t.add_row(vec![Cell::new("network"), Cell::new(&h.network)]);
        t.add_row(vec![Cell::new("template_id"), opt_cell(&h.template_id)]);
        t.add_row(vec![Cell::new("created_at"), Cell::new(&h.created_at)]);
        t.add_row(vec![Cell::new("updated_at"), opt_cell(&h.updated_at)]);
        t.add_row(vec![
            Cell::new("notification_email"),
            opt_cell(&h.notification_email),
        ]);
        if let Some(d) = &h.destination_attributes {
            t.add_row(vec![Cell::new("destination_attributes"), Cell::new(d)]);
        }
        write_table(w, &t)
    }
}

impl Render for quicknode_sdk::webhooks::WebhookEnabledCountResponse {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        writeln!(w, "{}", self.total)
    }
}
