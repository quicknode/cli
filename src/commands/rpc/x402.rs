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
use quicknode_sdk::{CreditBalance, PaymentConfig};

use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::{style, Style};

use super::payment::{
    ensure_gateway_session, resolve_payment_params, resolve_session_params, PaymentParams,
    SessionParams,
};

/// Query network used in the printed paid-lane examples. Payments and credits
/// are independent of the chain a call queries, so the examples query a chain
/// the user did not pay on to make that clear.
const EXAMPLE_QUERY_NETWORK: &str = "ethereum-mainnet";

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
#[command(after_help = "Examples:\n  \
    qn rpc x402 drip --payment-wallet payer\n  \
    qn rpc x402 balance --payment-wallet payer\n  \
    qn rpc x402 buy-credits --network ethereum-mainnet --payment-wallet payer \\\n      \
    --payment-network base-sepolia --payment-asset USDC --max-amount 10000000\n  \
    qn rpc call eth_blockNumber --network ethereum-mainnet --x402-drawdown --payment-wallet payer\n  \
    qn rpc x402 supported-networks\n  \
    qn rpc x402 supported-payments")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: X402Cmd,
}

#[derive(Debug, Subcommand)]
pub enum X402Cmd {
    /// Buy a block of prepaid credits, paying the gateway's offer with the
    /// configured wallet. Gated: names the spend ceiling before signing.
    #[command(after_help = "Examples:\n  \
        qn rpc x402 buy-credits --network ethereum-mainnet --payment-wallet payer \\\n      \
        --payment-network base-sepolia --payment-asset USDC --max-amount 10000000")]
    BuyCredits(PaymentArgs),

    /// Show the account's current credit balance (prints the bare number;
    /// --format json for the full envelope).
    #[command(visible_alias = "credits")]
    #[command(after_help = "Examples:\n  \
        qn rpc x402 balance --payment-wallet payer\n  \
        qn rpc x402 balance --payment-wallet payer --format json")]
    Balance(SessionArgs),

    /// Request testnet credits from the faucet (Base Sepolia, once per account).
    #[command(after_help = "Examples:\n  \
        qn rpc x402 drip --payment-wallet payer")]
    Drip(SessionArgs),

    /// List the networks you can make x402-paid RPC calls to. Each slug is a
    /// valid --network for a paid call. No API key required.
    #[command(visible_alias = "networks")]
    #[command(after_help = "Examples:\n  \
        qn rpc x402 networks\n  \
        qn rpc x402 supported-networks --format json")]
    SupportedNetworks,

    /// List the payment options the x402 gateway accepts: the network you pay
    /// on, the token, and its contract address — ready --payment-network and
    /// --payment-asset values. No API key required.
    #[command(visible_alias = "payments")]
    #[command(after_help = "Examples:\n  \
        qn rpc x402 payments\n  \
        qn rpc x402 supported-payments --format json")]
    SupportedPayments,
}

/// The shared payment parameter stack every x402 verb accepts, with the same
/// flags-then-`[rpc.payment]` resolution as `qn rpc call --x402`.
#[derive(Debug, ClapArgs)]
pub struct PaymentArgs {
    /// The gateway query chain to buy credits against, as its path slug (e.g.
    /// `base-sepolia`). Defaults to the `--payment-network` name when it is a
    /// slug. Credits are not network-scoped once bought.
    #[arg(long, value_name = "NETWORK")]
    pub network: Option<String>,

    /// File containing the raw payment private key (EVM hex); `-` reads stdin.
    /// Precedence: this > --payment-wallet > `key_file` > `wallet` in config.
    #[arg(long, value_name = "PATH", conflicts_with = "payment_wallet")]
    pub payment_key_file: Option<std::path::PathBuf>,

    /// Name of a stored wallet (from `qn wallet generate`) to pay with.
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

/// The flags a keyless gateway session accepts (`balance`, `drip`). These verbs
/// present a Bearer JWT and sign nothing, so they take only the wallet key and
/// the SIWX pay network — no `--payment-asset`, no `--max-amount`.
#[derive(Debug, ClapArgs)]
pub struct SessionArgs {
    /// The gateway query chain, as its path slug (e.g. `base-sepolia`).
    /// Defaults to the `--payment-network` name when it is a slug.
    #[arg(long, value_name = "NETWORK")]
    pub network: Option<String>,

    /// File containing the raw payment private key (EVM hex); `-` reads stdin.
    /// Precedence: this > --payment-wallet > `key_file` > `wallet` in config.
    #[arg(long, value_name = "PATH", conflicts_with = "payment_wallet")]
    pub payment_key_file: Option<std::path::PathBuf>,

    /// Name of a stored wallet (from `qn wallet generate`) to authenticate with.
    #[arg(long, value_name = "NAME")]
    pub payment_wallet: Option<String>,

    /// Chain the SIWX session authenticates on: a network name (e.g.
    /// `base-sepolia`) or CAIP-2 id (e.g. `eip155:84532`). Falls back to
    /// `payment_network` in [rpc.payment].
    #[arg(long, value_name = "NETWORK")]
    pub payment_network: Option<String>,

    /// Explicit Solana RPC URL for x402/Solana session auth. Falls back to
    /// `svm_rpc_url` in [rpc.payment], then a public Solana RPC.
    #[arg(long, value_name = "URL")]
    pub svm_rpc_url: Option<String>,
}

impl SessionArgs {
    fn params(&self) -> SessionParams<'_> {
        SessionParams {
            key_file: self.payment_key_file.as_deref(),
            wallet: self.payment_wallet.as_deref(),
            payment_network: self.payment_network.as_deref(),
            svm_rpc_url: self.svm_rpc_url.as_deref(),
        }
    }
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        X402Cmd::BuyCredits(a) => run_buy_credits(a, global).await,
        X402Cmd::Balance(a) => run_balance(a, global).await,
        X402Cmd::Drip(a) => run_drip(a, global).await,
        X402Cmd::SupportedNetworks => {
            super::supported_networks::run_networks(super::supported_networks::Scheme::X402, global)
                .await
        }
        X402Cmd::SupportedPayments => {
            super::supported_networks::run_payments(super::supported_networks::Scheme::X402, global)
                .await
        }
    }
}

// Resolve the full payment config (buy-credits) from the verb's args +
// [rpc.payment], build the keyless-payment Ctx, and print any key-file
// permissions warning once output exists. No network I/O yet.
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

// Setup for the session-only verbs (balance, drip): resolve the minimal
// wallet + pay-network config, build the keyless Ctx, warn on a loose key file.
// Signs nothing, so it needs no asset or spend ceiling.
fn session_setup(args: &SessionArgs, global: GlobalArgs) -> Result<Ctx, CliError> {
    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_session_params(
        &args.params(),
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;

    let ctx = Ctx::from_global_keyless_payment(global, payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }
    Ok(ctx)
}

// The gateway query chain (path slug) for a credit purchase: the explicit
// --network, else the --payment-network flag when it's a name (not a CAIP-2
// id). A resolved CAIP-2 payment network alone isn't a valid gateway slug, so
// require --network in that case.
fn resolve_query_network(args: &PaymentArgs, _payment: &PaymentConfig) -> Result<String, CliError> {
    if let Some(n) = &args.network {
        return Ok(n.clone());
    }
    if let Some(pn) = &args.payment_network {
        if !pn.contains(':') {
            return Ok(pn.clone());
        }
    }
    Err(CliError::Arg(
        "buy-credits needs the gateway query chain. Pass --network <SLUG> \
         (e.g. --network base-sepolia)."
            .to_string(),
    ))
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

async fn run_buy_credits(args: PaymentArgs, global: GlobalArgs) -> Result<(), CliError> {
    let (ctx, payment) = setup(&args, global.clone())?;

    // Gate Mild BEFORE any network I/O: name the spend ceiling and blast radius.
    // The exact charge is the gateway's offer, bounded by this ceiling; we name
    // the ceiling since a single-attempt purchase can't safely probe first.
    // Prompt in the user's vocabulary (symbol, network slug) where the tables
    // know it; fall back to the raw resolved values otherwise.
    let asset = super::pay_asset::symbol_for(&payment.pay_network, &payment.asset)
        .unwrap_or_else(|| payment.asset.clone());
    let pay_network = super::pay_network::slug_for_caip2(&payment.pay_network)
        .unwrap_or_else(|| payment.pay_network.clone());
    let msg = format!(
        "Buy credits for up to {} base units of {asset} on {pay_network}? \
         This moves real funds.",
        group_digits(&payment.max_amount)
    );
    crate::confirm::confirm_mild(&ctx, &msg)?;

    let network = resolve_query_network(&args, &payment)?;
    let session = ensure_gateway_session(&ctx, &global).await?;
    let balance = ctx
        .sdk
        .rpc
        .gateway_buy_credits(&session, &network)
        .await
        .map_err(super::payment::map_paid_error)?;

    ctx.out.note(&format!(
        "✓ Bought credits (balance: {})",
        fmt_credits(balance.credits)
    ));
    ctx.out.note(&format!(
        "\n{}\n\n{}",
        style(
            "Spend the credits on calls (1 credit per call, no per-call payment):",
            Style::Bold,
            ctx.out.color,
        ),
        drawdown_call_hint(&args, ctx.out.color)
    ));
    // Bare balance to stdout for pipelines; on a TTY the ✓ line above already
    // shows it, so skip the duplicate.
    if matches!(ctx.global.format, Some(f) if f.is_structured()) || !ctx.out.stdout_is_tty {
        return emit_balance(&ctx, &balance);
    }
    Ok(())
}

// A copy-pasteable, multi-line `--x402-drawdown` call with the wallet the user
// just paid with. Credits are not network-scoped, so the example deliberately
// queries a different chain than the one just bought against — it makes clear
// the credits spend on any supported network.
fn drawdown_call_hint(args: &PaymentArgs, color: bool) -> String {
    let mut cmd = format!(
        "  qn rpc call eth_blockNumber \\\n    \
           --network {EXAMPLE_QUERY_NETWORK} \\\n    \
           --x402-drawdown \\\n    \
           --payment-wallet {}",
        args.payment_wallet.as_deref().unwrap_or("<NAME>")
    );
    // The drawdown call defaults its pay network to --network, so keep the
    // SIWX auth chain on the network the user actually pays on when it differs.
    if let Some(pn) = &args.payment_network {
        if pn != EXAMPLE_QUERY_NETWORK {
            cmd.push_str(&format!(" \\\n    --payment-network {pn}"));
        }
    }
    style(&cmd, Style::Bold, color)
}

async fn run_balance(args: SessionArgs, global: GlobalArgs) -> Result<(), CliError> {
    let ctx = session_setup(&args, global.clone())?;
    let session = ensure_gateway_session(&ctx, &global).await?;
    let balance = ctx.sdk.rpc.gateway_credits(&session).await?;
    emit_balance(&ctx, &balance)
}

async fn run_drip(args: SessionArgs, global: GlobalArgs) -> Result<(), CliError> {
    let ctx = session_setup(&args, global.clone())?;
    let session = ensure_gateway_session(&ctx, &global).await?;
    let receipt = ctx.sdk.rpc.gateway_drip(&session).await?;

    // The faucet funds the wallet with testnet tokens (returns the funding tx),
    // which you then spend on buy-credits — it does not grant credits directly.
    ctx.out.note(&format!(
        "✓ Faucet funded {} (tx: {})",
        receipt.account_id, receipt.transaction_hash
    ));
    // Point at the two paid lanes with the flags the user already supplied;
    // USDC is the asset the faucet just funded. The examples query a chain the
    // faucet did not fund — payment is independent of the chain a call queries.
    let wallet = args.payment_wallet.as_deref().unwrap_or("<NAME>");
    let pay_net = args.payment_network.as_deref().unwrap_or("<NET>");
    let c = ctx.out.color;

    let mut block = String::new();
    block.push_str(
        "\nThis wallet now has funds that can be used to pay for blockchain calls\n\
         using micropayments.\n\n",
    );
    block.push_str(&style(
        "Pay per-request (sign a payment on each call):",
        Style::Bold,
        c,
    ));
    block.push_str(&format!(
        "\n\n{}\n\n",
        style(
            &format!(
                "  qn rpc call eth_blockNumber \\\n    \
                   --network {EXAMPLE_QUERY_NETWORK} \\\n    \
                   --x402 \\\n    \
                   --payment-wallet {wallet} \\\n    \
                   --payment-network {pay_net} \\\n    \
                   --payment-asset USDC \\\n    \
                   --max-amount 1000"
            ),
            Style::Bold,
            c,
        )
    ));
    block.push_str(&style(
        "Credit drawdown (buy prepaid credits once, then spend them):",
        Style::Bold,
        c,
    ));
    block.push_str(&format!(
        "\n\n{}\n\n{}",
        style(
            &format!(
                "  qn rpc x402 buy-credits \\\n    \
                   --network {EXAMPLE_QUERY_NETWORK} \\\n    \
                   --payment-wallet {wallet} \\\n    \
                   --payment-network {pay_net} \\\n    \
                   --payment-asset USDC \\\n    \
                   --max-amount 1000000"
            ),
            Style::Bold,
            c,
        ),
        // The explicit --payment-network keeps SIWX auth on the chain the
        // faucet funded, since a drawdown call defaults it to --network.
        style(
            &format!(
                "  qn rpc call eth_blockNumber \\\n    \
                   --network {EXAMPLE_QUERY_NETWORK} \\\n    \
                   --x402-drawdown \\\n    \
                   --payment-wallet {wallet} \\\n    \
                   --payment-network {pay_net}"
            ),
            Style::Bold,
            c,
        )
    ));
    ctx.out.note(&block);
    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let v = serde_json::json!({
            "account_id": receipt.account_id,
            "transaction_hash": receipt.transaction_hash,
        });
        return super::emit_result(&ctx, &v);
    }
    // Bare tx hash to stdout for pipelines; on a TTY the ✓ line above already
    // shows it, so skip the duplicate.
    if !ctx.out.stdout_is_tty {
        println!("{}", receipt.transaction_hash);
    }
    Ok(())
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
    group_digits(&n.to_string())
}

// Digit-group a decimal string (1000000 -> 1,000,000). Anything that is not
// pure ASCII digits passes through unchanged rather than being mangled.
fn group_digits(s: &str) -> String {
    let bytes = s.as_bytes();
    if s.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return s.to_string();
    }
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

    #[test]
    fn group_digits_leaves_non_digit_strings_alone() {
        assert_eq!(group_digits("1000000"), "1,000,000");
        assert_eq!(group_digits(""), "");
        assert_eq!(group_digits("+5000"), "+5000");
        assert_eq!(group_digits("0xabc"), "0xabc");
    }
}
