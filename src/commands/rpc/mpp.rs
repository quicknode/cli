//! `qn rpc mpp …` — the MPP payment-channel (session) lifecycle.
//!
//! Four verbs manage an on-chain escrow payment channel, all keyless (funded by
//! the configured Tempo wallet, not an account API key):
//! - `open`    — deposit into the escrow, opening a channel. Gated Mild.
//! - `top-up`  — add deposit to the open channel. Gated Mild.
//! - `close`   — cooperatively close (settle on-chain + refund). Gated Mild;
//!   the prompt warns that further `--mpp-session` calls fail until re-open.
//! - `status`  — the gateway's view of the channel (also the recovery path
//!   when local channel state is lost).
//!
//! Channel state (channelId, deposit, cumulative spend) is persisted under the
//! config dir (`channels.toml`, 0600, keyed by wallet address + network) and
//! re-seeded next run. Every lifecycle verb ends with a ready-to-run next
//! command. Paid verbs are single-attempt.

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::ChannelState;

use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;

use super::payment::{resolve_payment_params, PaymentParams};

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: MppCmd,
}

#[derive(Debug, Subcommand)]
pub enum MppCmd {
    /// Open a payment channel by depositing into the escrow. Gated: names the
    /// deposit before signing.
    #[command(after_help = "Examples:\n  \
        qn rpc mpp open --network tempo-testnet --deposit 1000000 \\\n      \
        --payment-wallet payer --payment-network tempo-testnet --payment-asset USDC")]
    Open(OpenArgs),

    /// Add deposit to the open channel. Gated.
    #[command(name = "top-up")]
    TopUp(TopUpArgs),

    /// Cooperatively close the channel (settle on-chain + refund unused
    /// deposit). Gated.
    Close(ChannelArgs),

    /// Show the gateway's view of the channel (also the state-recovery path).
    Status(ChannelArgs),
}

/// The payment parameter stack + query network shared by the MPP verbs.
#[derive(Debug, ClapArgs)]
pub struct PaymentArgs {
    /// The query chain, as the payment gateway's path slug (e.g.
    /// `tempo-testnet`). The channel lives on this network.
    #[arg(long, value_name = "NETWORK")]
    pub network: String,

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

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        MppCmd::Open(a) => run_open(a, global).await,
        MppCmd::TopUp(a) => run_top_up(a, global).await,
        MppCmd::Close(a) => run_close(a.payment, global).await,
        MppCmd::Status(a) => run_status(a.payment, global).await,
    }
}

// Shared setup: resolve the MPP payment config (scheme "mpp"), build the
// keyless-payment Ctx, and surface any key-file warning. No network I/O.
fn setup(args: &PaymentArgs, global: GlobalArgs) -> Result<(Ctx, String), CliError> {
    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_payment_params(
        "mpp",
        &args.params(),
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;
    let network = args.network.clone();
    let ctx = Ctx::from_global_keyless_payment(global, payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }
    Ok((ctx, network))
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

async fn run_open(args: OpenArgs, global: GlobalArgs) -> Result<(), CliError> {
    let deposit = parse_base_units(&args.deposit, "deposit")?;
    let (ctx, network) = setup(&args.payment, global.clone())?;

    crate::confirm::confirm_mild(
        &ctx,
        &format!(
            "Open an MPP channel with a {deposit} base-unit deposit on {network}? \
             This moves real funds on-chain."
        ),
    )?;

    let channel = ctx
        .sdk
        .rpc
        .mpp_open(&network, deposit)
        .await
        .map_err(super::payment::map_paid_error)?;
    persist_channel(&ctx, &global, &network, &channel);

    ctx.out.note(&format!(
        "✓ Opened channel {} (deposit: {})",
        channel.channel_id, channel.deposit
    ));
    ctx.out.note(&format!(
        "  Next: qn rpc call eth_blockNumber --network {network} --mpp-session"
    ));
    emit_channel(&ctx, &channel)
}

async fn run_top_up(args: TopUpArgs, global: GlobalArgs) -> Result<(), CliError> {
    let additional = parse_base_units(&args.deposit, "deposit")?;
    let (ctx, network) = setup(&args.payment, global.clone())?;
    let channel = require_channel(&ctx, &global, &network)?;

    crate::confirm::confirm_mild(
        &ctx,
        &format!(
            "Top up channel {} with {additional} more base units on {network}? \
             This moves real funds on-chain.",
            channel.channel_id
        ),
    )?;

    let updated = ctx
        .sdk
        .rpc
        .mpp_top_up(&network, &channel, additional)
        .await
        .map_err(super::payment::map_paid_error)?;
    persist_channel(&ctx, &global, &network, &updated);

    ctx.out.note(&format!(
        "✓ Topped up channel {} (deposit: {})",
        updated.channel_id, updated.deposit
    ));
    ctx.out
        .note(&format!("  Next: qn rpc mpp status --network {network}"));
    emit_channel(&ctx, &updated)
}

async fn run_close(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, network) = setup(&args, global.clone())?;
    let channel = require_channel(&ctx, &global, &network)?;

    crate::confirm::confirm_mild(
        &ctx,
        &format!(
            "Close channel {} on {network}? It settles on-chain and refunds the \
             unused deposit; further --mpp-session calls fail until you open a \
             new channel.",
            channel.channel_id
        ),
    )?;

    ctx.sdk
        .rpc
        .mpp_close(&network, &channel)
        .await
        .map_err(super::payment::map_paid_error)?;
    // The channel is settled: drop the local record so a stale one can't be
    // reused. Best-effort — a failed delete must not fail the completed close.
    if let Some(address) = wallet_address(&ctx) {
        if let Some(path) = config::channels_cache_path(global.resolve_config_path().as_deref()) {
            let _ = config::delete_channel(&path, &address, &network);
        }
    }

    ctx.out
        .note(&format!("✓ Closed channel {}", channel.channel_id));
    ctx.out.note(&format!(
        "  Next: qn rpc mpp open --network {network} --deposit <BASE_UNITS>"
    ));
    Ok(())
}

async fn run_status(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, network) = setup(&args, global.clone())?;
    let channel = require_channel(&ctx, &global, &network)?;

    let status = ctx
        .sdk
        .rpc
        .mpp_status(&network, &channel.channel_id)
        .await?;

    // Re-seed local state from the gateway's high-water mark (the recovery
    // path): the gateway is authoritative for deposit + accepted cumulative.
    let mut synced = channel.clone();
    synced.deposit = status.deposit;
    synced.cumulative_spent = status.accepted_cumulative;
    persist_channel(&ctx, &global, &network, &synced);

    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let v = serde_json::json!({
            "channel_id": status.channel_id,
            "deposit": status.deposit,
            "accepted_cumulative": status.accepted_cumulative,
            "remaining": status.deposit.saturating_sub(status.accepted_cumulative),
        });
        return super::emit_result(&ctx, &v);
    }
    ctx.out.note(&format!(
        "channel {}: deposit {}, spent {}, remaining {}",
        status.channel_id,
        status.deposit,
        status.accepted_cumulative,
        status.deposit.saturating_sub(status.accepted_cumulative),
    ));
    Ok(())
}

// The wallet's on-chain address (derived offline), used to key channel state.
fn wallet_address(ctx: &Ctx) -> Option<String> {
    ctx.sdk.rpc.payment_address().ok()
}

// Persist channel state, keyed by wallet address + network. Best-effort.
fn persist_channel(ctx: &Ctx, global: &GlobalArgs, network: &str, channel: &ChannelState) {
    if let Some(address) = wallet_address(ctx) {
        if let Some(path) = config::channels_cache_path(global.resolve_config_path().as_deref()) {
            let _ = config::save_channel(&path, &address, network, channel);
        }
    }
}

// Load the open channel for this wallet+network, or an actionable error
// pointing at `mpp open`. A lost local record is recovered by re-opening or,
// if the channel is known, by `mpp status` (which re-seeds it).
fn require_channel(
    ctx: &Ctx,
    global: &GlobalArgs,
    network: &str,
) -> Result<ChannelState, CliError> {
    let address = wallet_address(ctx)
        .ok_or_else(|| CliError::Arg("could not derive the payment wallet address".to_string()))?;
    let path = config::channels_cache_path(global.resolve_config_path().as_deref());
    let channel = path
        .as_deref()
        .and_then(|p| config::load_channel(p, &address, network));
    channel.ok_or_else(|| {
        CliError::Arg(format!(
            "no open MPP channel for this wallet on {network}. Open one with \
             'qn rpc mpp open --network {network} --deposit <BASE_UNITS>'."
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
