//! `qn rpc x402 …` — the x402 credit-drawdown gateway lifecycle.
//!
//! Three verbs manage prepaid gateway credits, all keyless (paid by the
//! configured wallet, not an account API key):
//! - `buy-credits` — SIWX-authenticate, then settle the gateway's 402 credit
//!   offer with the payment wallet. Gated Mild (it moves funds).
//! - `balance`     — GET the account's current credit balance.
//! - `drip`        — testnet faucet (Base Sepolia, once per account).
//!
//! The session JWT is authenticated on first use, cached under the config dir
//! (`sessions.toml`, 0600, keyed by the payer wallet's address), and re-seeded
//! next run — the same seed/export pattern as the Tooling Access token. A
//! missing/expired session re-authenticates transparently: it moves no funds,
//! so it needs no confirmation.
//!
//! Every verb ends its success output with a ready-to-run next command
//! (stderr via `ctx.out.note`) so the drawdown flow chains without the user
//! hunting for the next step. Paid verbs are single-attempt: a paid lane never
//! blind-retries.

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::{CreditBalance, GatewaySession, PaymentConfig};

use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;

use super::payment::{resolve_payment_params, PaymentParams};

// Re-auth margin: treat a session expiring within this window as stale and mint
// a fresh one, absorbing clock skew. Mirrors the tooling token's 60s margin.
const SESSION_MARGIN_SECS: i64 = 60;

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: X402Cmd,
}

#[derive(Debug, Subcommand)]
pub enum X402Cmd {
    /// Buy a block of prepaid credits, paying the gateway's offer with the
    /// configured wallet. Gated: names the spend ceiling before signing.
    #[command(after_help = "Examples:\n  \
        qn rpc x402 buy-credits --payment-wallet payer \\\n      \
        --payment-network base-sepolia --payment-asset USDC --max-amount 10000000")]
    BuyCredits(PaymentArgs),

    /// Show the account's current credit balance (prints the bare number;
    /// --format json for the full envelope).
    #[command(visible_alias = "credits")]
    Balance(PaymentArgs),

    /// Request testnet credits from the faucet (Base Sepolia, once per account).
    Drip(PaymentArgs),
}

/// The shared payment parameter stack every x402 verb accepts, with the same
/// flags-then-`[rpc.payment]` resolution as `qn rpc call --x402`.
#[derive(Debug, ClapArgs)]
pub struct PaymentArgs {
    /// File containing the raw payment private key (EVM hex); `-` reads stdin.
    /// Precedence: this > --payment-wallet > `key_file` > `wallet` in config.
    #[arg(long, value_name = "PATH", conflicts_with = "payment_wallet")]
    pub payment_key_file: Option<std::path::PathBuf>,

    /// Name of a stored wallet (from `qn rpc wallet generate`) to pay with.
    #[arg(long, value_name = "NAME")]
    pub payment_wallet: Option<String>,

    /// Spend ceiling for a credit purchase, in integer base units of the asset
    /// (e.g. 10000000 = 10 USDC). Flag > `max_amount` in [rpc.payment]. An
    /// offered purchase above this is never signed.
    #[arg(long, value_name = "BASE_UNITS")]
    pub max_amount: Option<String>,

    /// Chain you pay on: a network name (e.g. `base-sepolia`) or CAIP-2 id
    /// (e.g. `eip155:84532`). Falls back to `payment_network` in [rpc.payment].
    #[arg(long, value_name = "NETWORK")]
    pub payment_network: Option<String>,

    /// Token to pay with: an EVM contract address or a symbol like USDC
    /// (resolved per network). Falls back to `payment_asset` in [rpc.payment].
    #[arg(long, value_name = "ADDRESS")]
    pub payment_asset: Option<String>,

    /// Explicit Solana RPC URL for x402/Solana payment builds. Falls back to
    /// `svm_rpc_url` in [rpc.payment], then a public Solana RPC.
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

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        X402Cmd::BuyCredits(a) => run_buy_credits(a, global).await,
        X402Cmd::Balance(a) => run_balance(a, global).await,
        X402Cmd::Drip(a) => run_drip(a, global).await,
    }
}

// Resolve the payment config from the verb's args + [rpc.payment], build the
// keyless-payment Ctx, and print any key-file permissions warning once output
// exists. Shared setup for all three verbs — no network I/O yet.
fn setup(args: &PaymentArgs, global: GlobalArgs) -> Result<(Ctx, PaymentConfig), CliError> {
    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_payment_params(
        "x402",
        &args.params(),
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;

    let ctx = Ctx::from_global_keyless_payment(global, payment.clone())?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }
    Ok((ctx, payment))
}

/// Loads `[rpc.payment]`. A missing file is an empty section; an unreadable or
/// invalid file is a hard error (the user likely relies on values set there).
fn load_payment_section(global: &GlobalArgs) -> Result<PaymentSection, CliError> {
    let Some(path) = global.resolve_config_path() else {
        return Ok(PaymentSection::default());
    };
    Ok(config::load_from(&path)?
        .map(|cfg| cfg.rpc.payment)
        .unwrap_or_default())
}

// Return a valid gateway session for the configured wallet, authenticating
// (and caching) when there's no fresh cached JWT. Free — no funds move — so no
// confirmation. The cache is keyed by the wallet's on-chain address, derived
// offline, so the lookup is a single local read; a hit skips the SIWX round
// trip, a miss (or an expired session) authenticates once and re-caches.
async fn ensure_session(ctx: &Ctx, global: &GlobalArgs) -> Result<GatewaySession, CliError> {
    let sessions_path = config::sessions_cache_path(global.resolve_config_path().as_deref());
    let address = ctx.sdk.rpc.payment_address()?;

    if let Some(path) = &sessions_path {
        if let Some(existing) = config::load_gateway_session_by_address(path, &address) {
            if existing.is_fresh(SESSION_MARGIN_SECS) {
                return Ok(existing);
            }
        }
    }

    let session = ctx.sdk.rpc.gateway_authenticate().await?;
    if let Some(path) = &sessions_path {
        let _ = config::save_gateway_session(path, &address, &session);
    }
    Ok(session)
}

async fn run_buy_credits(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, payment) = setup(&args, global.clone())?;

    // Gate Mild BEFORE any network I/O: name the spend ceiling and blast radius.
    // The exact charge is the gateway's offer, bounded by this ceiling; we name
    // the ceiling since a single-attempt purchase can't safely probe first.
    let msg = format!(
        "Buy credits for up to {} base units of {} on {}? This moves real funds.",
        payment.max_amount, payment.asset, payment.pay_network
    );
    crate::confirm::confirm_mild(&ctx, &msg)?;

    let session = ensure_session(&ctx, &global).await?;
    let balance = ctx
        .sdk
        .rpc
        .gateway_buy_credits(&session)
        .await
        .map_err(super::payment::map_paid_error)?;

    ctx.out.note(&format!(
        "✓ Bought credits (balance: {})",
        fmt_credits(balance.credits)
    ));
    ctx.out
        .note("  Next: qn rpc call eth_blockNumber --network base-sepolia --x402-drawdown");
    emit_balance(&ctx, &balance)
}

async fn run_balance(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, _payment) = setup(&args, global.clone())?;
    let session = ensure_session(&ctx, &global).await?;
    let balance = ctx.sdk.rpc.gateway_credits(&session).await?;
    emit_balance(&ctx, &balance)
}

async fn run_drip(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, _payment) = setup(&args, global.clone())?;
    let session = ensure_session(&ctx, &global).await?;
    let balance = ctx.sdk.rpc.gateway_drip(&session).await?;

    ctx.out.note(&format!(
        "✓ Dripped testnet credits (balance: {})",
        fmt_credits(balance.credits)
    ));
    ctx.out
        .note("  Next: qn rpc call eth_blockNumber --network base-sepolia --x402-drawdown");
    emit_balance(&ctx, &balance)
}

// Emit a credit balance: the bare number by default (friendly for scripts and
// pipelines, TTY or piped), the full envelope only for a structured --format.
fn emit_balance(ctx: &Ctx, balance: &CreditBalance) -> Result<(), CliError> {
    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let v = serde_json::json!({
            "account_id": balance.account_id,
            "credits": balance.credits,
        });
        return super::emit_result(ctx, &v);
    }
    println!("{}", balance.credits);
    Ok(())
}

// Group digits for the human-facing note (1000000 -> 1,000,000). The bare
// stdout number is never grouped, so pipelines get a clean integer.
fn fmt_credits(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_credits_groups_thousands() {
        assert_eq!(fmt_credits(0), "0");
        assert_eq!(fmt_credits(95), "95");
        assert_eq!(fmt_credits(1_000), "1,000");
        assert_eq!(fmt_credits(1_000_095), "1,000,095");
    }
}
