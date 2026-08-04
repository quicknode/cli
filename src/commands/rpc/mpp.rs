//! MPP payment-channel lifecycle. Channel state is cached by payer, pay network,
//! and asset. Lifecycle operations are keyless and single-attempt.

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::ChannelState;

use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::{style, Style};

use super::payment::{resolve_payment_params, PaymentParams};

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
#[command(after_help = "Examples:\n  \
    qn rpc mpp open --deposit 1000000 \\\n      \
    --payment-wallet payer --payment-network tempo-testnet \\\n      \
    --payment-asset pathUSD --max-amount 1000000\n  \
    qn rpc call eth_blockNumber --network ethereum-mainnet --mpp-session \\\n      \
    --payment-wallet payer --payment-network tempo-testnet \\\n      \
    --payment-asset pathUSD --max-amount 1000000\n  \
    qn rpc mpp status \\\n      \
    --payment-wallet payer --payment-network tempo-testnet \\\n      \
    --payment-asset pathUSD --max-amount 1000000\n  \
    qn rpc mpp top-up --deposit 500000 \\\n      \
    --payment-wallet payer --payment-network tempo-testnet \\\n      \
    --payment-asset pathUSD --max-amount 500000\n  \
    qn rpc mpp close \\\n      \
    --payment-wallet payer --payment-network tempo-testnet \\\n      \
    --payment-asset pathUSD --max-amount 1000000")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: MppCmd,
}

#[derive(Debug, Subcommand)]
pub enum MppCmd {
    /// Open a payment channel by depositing into the escrow. Gated: names the
    /// deposit before signing.
    #[command(after_help = "Examples:\n  \
        qn rpc mpp open --deposit 1000000 \\\n      \
        --payment-wallet payer --payment-network tempo-testnet --payment-asset USDC \\\n      \
        --max-amount 1000000")]
    Open(OpenArgs),

    /// Add deposit to the open channel. Gated.
    #[command(name = "top-up")]
    #[command(after_help = "Examples:\n  \
        qn rpc mpp top-up --deposit 500000 \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset pathUSD --max-amount 500000")]
    TopUp(TopUpArgs),

    /// Cooperatively close the channel (settle on-chain + refund unused
    /// deposit). Gated.
    #[command(after_help = "Examples:\n  \
        qn rpc mpp close \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset pathUSD --max-amount 1000000")]
    Close(ChannelArgs),

    /// Show the channel's deposit and remaining balance from the local record.
    /// Pass --verify to ask the gateway instead (spends one request unit).
    #[command(after_help = "Examples:\n  \
        qn rpc mpp status \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset pathUSD --max-amount 1000000\n  \
        qn rpc mpp status --verify \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset pathUSD --max-amount 1000000")]
    Status(StatusArgs),

    /// List the networks you can make MPP-paid RPC calls to. Each slug is a
    /// valid --network for a paid call. No API key required.
    #[command(visible_alias = "networks")]
    #[command(after_help = "Examples:\n  \
        qn rpc mpp networks\n  \
        qn rpc mpp supported-networks --format json")]
    SupportedNetworks,

    /// List the payment options the MPP gateway accepts: the network you pay
    /// on, the token, and its contract address — ready --payment-network and
    /// --payment-asset values. No API key required.
    #[command(visible_alias = "payments")]
    #[command(after_help = "Examples:\n  \
        qn rpc mpp payments\n  \
        qn rpc mpp supported-payments --format json")]
    SupportedPayments,
}

/// The payment parameter stack shared by the MPP verbs. There is no query
/// network here: the channel is scoped by --payment-network and
/// --payment-asset, and one open channel funds paid calls to every supported
/// network, so the lifecycle verbs never name a chain to query.
#[derive(Debug, ClapArgs)]
pub struct PaymentArgs {
    /// File containing the raw Tempo payment key (hex); `-` reads stdin.
    /// Precedence: this > --payment-wallet > `key_file` > `wallet` in config.
    #[arg(long, value_name = "PATH", conflicts_with = "payment_wallet")]
    pub payment_key_file: Option<std::path::PathBuf>,

    /// Name of a stored wallet (from `qn wallet generate`) to pay with.
    #[arg(long, value_name = "NAME")]
    pub payment_wallet: Option<String>,

    /// Spend ceiling for a single signed action (deposit/top-up), in integer
    /// base units. Flag > `max_amount` in [rpc.payment].
    #[arg(long, value_name = "BASE_UNITS")]
    pub max_amount: Option<String>,

    /// Chain the channel settles on: a network name or CAIP-2 id. Falls back to
    /// `payment_network` in [rpc.payment].
    #[arg(long, value_name = "NETWORK")]
    pub payment_network: Option<String>,

    /// Token the channel is denominated in: an address or a symbol like USDC.
    /// Falls back to `payment_asset` in [rpc.payment].
    #[arg(long, value_name = "ADDRESS")]
    pub payment_asset: Option<String>,

    /// Explicit Solana RPC URL (unused for Tempo channels; accepted for a
    /// uniform flag stack). Falls back to `svm_rpc_url` in [rpc.payment].
    #[arg(long, value_name = "URL")]
    pub svm_rpc_url: Option<String>,
}

impl PaymentArgs {
    fn params(&self) -> PaymentParams<'_> {
        PaymentParams {
            key_file: self.payment_key_file.as_deref(),
            wallet: self.payment_wallet.as_deref(),
            max_amount: self.max_amount.as_deref(),
            payment_network: self.payment_network.as_deref(),
            payment_asset: self.payment_asset.as_deref(),
            svm_rpc_url: self.svm_rpc_url.as_deref(),
        }
    }
}

#[derive(Debug, ClapArgs)]
pub struct OpenArgs {
    #[command(flatten)]
    pub payment: PaymentArgs,

    /// Amount to deposit into the escrow, in integer base units of the asset.
    #[arg(long, value_name = "BASE_UNITS")]
    pub deposit: String,
}

#[derive(Debug, ClapArgs)]
pub struct TopUpArgs {
    #[command(flatten)]
    pub payment: PaymentArgs,

    /// Additional amount to deposit into the open channel, in base units.
    #[arg(long, value_name = "BASE_UNITS")]
    pub deposit: String,
}

/// Verbs that operate on the already-open channel (no new deposit amount).
#[derive(Debug, ClapArgs)]
pub struct ChannelArgs {
    #[command(flatten)]
    pub payment: PaymentArgs,
}

/// `status` reads the local channel record by default. `--verify` asks the
/// gateway instead, which costs one request unit (see `run_status`).
#[derive(Debug, ClapArgs)]
pub struct StatusArgs {
    #[command(flatten)]
    pub payment: PaymentArgs,

    /// Ask the gateway for its view instead of reading the local record. Spends
    /// one request unit from the channel deposit.
    #[arg(long)]
    pub verify: bool,
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        MppCmd::Open(a) => run_open(a, global).await,
        MppCmd::TopUp(a) => run_top_up(a, global).await,
        MppCmd::Close(a) => run_close(a.payment, global).await,
        MppCmd::Status(a) => run_status(a, global).await,
        MppCmd::SupportedNetworks => {
            super::supported_networks::run_networks(super::supported_networks::Scheme::Mpp, global)
                .await
        }
        MppCmd::SupportedPayments => {
            super::supported_networks::run_payments(super::supported_networks::Scheme::Mpp, global)
                .await
        }
    }
}

// Resolve payment config and the channel's pay scope before network I/O.
fn setup(args: &PaymentArgs, global: GlobalArgs) -> Result<(Ctx, PayScope), CliError> {
    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_payment_params(
        "mpp",
        &args.params(),
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;
    let scope = PayScope::from_config(&payment);
    let ctx = Ctx::from_global_keyless_payment(global, payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }
    Ok((ctx, scope))
}

// Pay scope carried to channel-cache keying; the payer address is added later.
pub(super) struct PayScope {
    pay_network: String,
    pay_asset: String,
}

impl PayScope {
    pub(super) fn from_config(payment: &quicknode_sdk::PaymentConfig) -> Self {
        PayScope {
            pay_network: payment.pay_network.clone(),
            pay_asset: payment.asset.clone(),
        }
    }

    pub(super) fn with_address(&self, address: String) -> config::ChannelScope {
        config::ChannelScope {
            address,
            pay_network: self.pay_network.clone(),
            pay_asset: self.pay_asset.clone(),
        }
    }

    pub(super) fn describe(&self) -> String {
        format!("{} on {}", self.pay_asset, self.pay_network)
    }
}

fn load_payment_section(global: &GlobalArgs) -> Result<PaymentSection, CliError> {
    let Some(path) = global.resolve_config_path() else {
        return Ok(PaymentSection::default());
    };
    Ok(config::load_from(&path)?
        .map(|cfg| cfg.rpc.payment)
        .unwrap_or_default())
}

fn parse_base_units(s: &str, flag: &str) -> Result<u128, CliError> {
    s.parse::<u128>().map_err(|_| {
        CliError::Arg(format!(
            "--{flag} must be a non-negative integer in base units, got '{s}'"
        ))
    })
}

// Print a ready-to-run next-command hint using explicit payment flags.
fn note_next(ctx: &Ctx, base: &str, payment: &PaymentArgs) {
    let mut flags: Vec<String> = Vec::new();
    if let Some(f) = &payment.payment_key_file {
        flags.push(format!("--payment-key-file {}", f.display()));
    }
    if let Some(w) = &payment.payment_wallet {
        flags.push(format!("--payment-wallet {w}"));
    }
    if let Some(n) = &payment.payment_network {
        flags.push(format!("--payment-network {n}"));
    }
    if let Some(a) = &payment.payment_asset {
        flags.push(format!("--payment-asset {a}"));
    }
    if let Some(m) = &payment.max_amount {
        flags.push(format!("--max-amount {m}"));
    }

    let mut lines = vec![format!("  {base}")];
    for chunk in flags.chunks(2) {
        lines.push(format!("    {}", chunk.join(" ")));
    }
    let cmd = lines.join(" \\\n");
    ctx.out.note(&format!(
        "\n{}\n\n{}\n",
        style("Example", Style::Bold, ctx.out.color),
        style(&cmd, Style::Bold, ctx.out.color),
    ));
}

async fn run_open(args: OpenArgs, global: GlobalArgs) -> Result<(), CliError> {
    let deposit = parse_base_units(&args.deposit, "deposit")?;
    let (ctx, scope) = setup(&args.payment, global.clone())?;

    crate::confirm::confirm_mild(
        &ctx,
        &format!(
            "Open an MPP channel with a {deposit} base-unit deposit of {}? \
             This moves real funds on-chain.",
            scope.describe()
        ),
    )?;

    let channel = ctx
        .sdk
        .rpc
        .mpp_open(deposit)
        .await
        .map_err(super::payment::map_paid_error)?;
    persist_channel(&ctx, &global, &scope, &channel);

    ctx.out.note(&format!(
        "✓ Opened channel {} (deposit: {})",
        channel.channel_id, channel.deposit
    ));
    note_next(
        &ctx,
        "qn rpc call eth_blockNumber --network ethereum-mainnet --mpp-session",
        &args.payment,
    );
    emit_channel(&ctx, &channel)
}

async fn run_top_up(args: TopUpArgs, global: GlobalArgs) -> Result<(), CliError> {
    let additional = parse_base_units(&args.deposit, "deposit")?;
    let (ctx, scope) = setup(&args.payment, global.clone())?;
    let channel = require_channel(&ctx, &global, &scope)?;

    crate::confirm::confirm_mild(
        &ctx,
        &format!(
            "Top up channel {} with {additional} more base units of {}? \
             This moves real funds on-chain.",
            channel.channel_id,
            scope.describe()
        ),
    )?;

    let updated = ctx
        .sdk
        .rpc
        .mpp_top_up(&channel, additional)
        .await
        .map_err(super::payment::map_paid_error)?;
    persist_channel(&ctx, &global, &scope, &updated);

    ctx.out.note(&format!(
        "✓ Topped up channel {} (deposit: {})",
        updated.channel_id, updated.deposit
    ));
    note_next(&ctx, "qn rpc mpp status", &args.payment);
    emit_channel(&ctx, &updated)
}

async fn run_close(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, scope) = setup(&args, global.clone())?;
    let channel = require_channel(&ctx, &global, &scope)?;

    crate::confirm::confirm_mild(
        &ctx,
        &format!(
            "Close channel {} ({})? It settles on-chain and refunds the \
             unused deposit; further --mpp-session calls fail until you open a \
             new channel.",
            channel.channel_id,
            scope.describe()
        ),
    )?;

    ctx.sdk
        .rpc
        .mpp_close(&channel)
        .await
        .map_err(super::payment::map_paid_error)?;
    // Do not reuse a channel after settlement.
    if let Some(address) = wallet_address(&ctx) {
        if let Some(path) = config::channels_cache_path(global.resolve_config_path().as_deref()) {
            let _ = config::delete_channel(&path, &scope.with_address(address));
        }
    }

    ctx.out
        .note(&format!("✓ Closed channel {}", channel.channel_id));
    note_next(&ctx, "qn rpc mpp open --deposit <BASE_UNITS>", &args);
    Ok(())
}

// Read local state by default; verification spends one request unit and syncs it.
async fn run_status(args: StatusArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, scope) = setup(&args.payment, global.clone())?;
    let channel = require_channel(&ctx, &global, &scope)?;

    if !args.verify {
        return emit_status(&ctx, &channel, channel.cumulative_spent, None);
    }

    let status = ctx.sdk.rpc.mpp_status(&channel).await?;

    // Persist the gateway's accepted high-water mark before rendering.
    let mut synced = channel.clone();
    synced.cumulative_spent = synced
        .cumulative_spent
        .saturating_add(synced.per_call)
        .max(status.accepted_cumulative);
    persist_channel(&ctx, &global, &scope, &synced);

    emit_status(
        &ctx,
        &synced,
        status.accepted_cumulative,
        Some(status.spent),
    )
}

fn emit_status(
    ctx: &Ctx,
    channel: &ChannelState,
    accepted: u128,
    spent: Option<u128>,
) -> Result<(), CliError> {
    let deposit = channel.deposit;
    let remaining = deposit.saturating_sub(accepted);
    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let mut v = serde_json::json!({
            "channel_id": channel.channel_id,
            "deposit": deposit,
            "accepted_cumulative": accepted,
            "remaining": remaining,
            "verified": spent.is_some(),
        });
        if let Some(spent) = spent {
            v["spent"] = serde_json::json!(spent);
        }
        return super::emit_result(ctx, &v);
    }
    ctx.out.note(&format!(
        "channel {}: deposit {}, accepted {}, remaining {}{}",
        channel.channel_id,
        deposit,
        accepted,
        remaining,
        if spent.is_some() {
            ""
        } else {
            " (local record; --verify asks the gateway)"
        },
    ));
    Ok(())
}

fn wallet_address(ctx: &Ctx) -> Option<String> {
    ctx.sdk.rpc.payment_address().ok()
}

fn persist_channel(ctx: &Ctx, global: &GlobalArgs, scope: &PayScope, channel: &ChannelState) {
    if let Some(address) = wallet_address(ctx) {
        if let Some(path) = config::channels_cache_path(global.resolve_config_path().as_deref()) {
            let _ = config::save_channel(&path, &scope.with_address(address), channel);
        }
    }
}

fn require_channel(
    ctx: &Ctx,
    global: &GlobalArgs,
    scope: &PayScope,
) -> Result<ChannelState, CliError> {
    let address = wallet_address(ctx)
        .ok_or_else(|| CliError::Arg("could not derive the payment wallet address".to_string()))?;
    let path = config::channels_cache_path(global.resolve_config_path().as_deref());
    let channel = path
        .as_deref()
        .and_then(|p| config::load_channel(p, &scope.with_address(address)));
    channel.ok_or_else(|| {
        CliError::Arg(format!(
            "no open MPP channel for this wallet paying {}. Open one with \
             'qn rpc mpp open --deposit <BASE_UNITS>'.",
            scope.describe()
        ))
    })
}

fn emit_channel(ctx: &Ctx, channel: &ChannelState) -> Result<(), CliError> {
    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let v = serde_json::json!({
            "channel_id": channel.channel_id,
            "deposit": channel.deposit,
            "cumulative_spent": channel.cumulative_spent,
        });
        return super::emit_result(ctx, &v);
    }
    println!("{}", channel.channel_id);
    Ok(())
}
