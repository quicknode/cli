//! x402 prepaid-credit lifecycle. Sessions are cached by payer address and
//! paid operations are single-attempt.

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

// Keep examples explicit about query and payment networks being independent.
const EXAMPLE_QUERY_NETWORK: &str = "ethereum-mainnet";

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
#[command(after_help = "Examples:\n  \
    qn rpc x402 drip --payment-wallet payer --payment-network base-sepolia\n  \
    qn rpc x402 balance --payment-wallet payer --payment-network base-sepolia\n  \
    qn rpc x402 buy-credits --network ethereum-mainnet --payment-wallet payer \\\n      \
    --payment-network base-sepolia --payment-asset USDC --max-amount 10000000\n  \
     qn rpc call eth_blockNumber --network ethereum-mainnet --x402-drawdown --payment-wallet payer\n  \
     qn rpc x402 buy-credits --network solana-devnet --payment-wallet sol-payer \\\n      \
     --payment-network solana-devnet --payment-asset 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU \\\n     \
      --max-amount 1000000 --svm-rpc-url https://solana.example/rpc\n  \
     qn rpc call getSlot --network solana-devnet --x402-drawdown \\\n     \
      --payment-wallet sol-payer --payment-network solana-devnet\n  \
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
        qn rpc x402 balance --payment-wallet payer --payment-network base-sepolia\n  \
        qn rpc x402 balance --payment-wallet payer --payment-network base-sepolia \\\n      \
        --format json")]
    Balance(SessionArgs),

    /// Request testnet credits from the faucet (Base Sepolia, once per account).
    #[command(after_help = "Examples:\n  \
        qn rpc x402 drip --payment-wallet payer --payment-network base-sepolia")]
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

    /// File containing the raw payment private key (EVM hex or Solana base58);
    /// `-` reads stdin.
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

    /// Token to pay with: an EVM contract address, Solana mint, or a symbol like
    /// USDC (resolved per network). Falls back to `payment_asset` in [rpc.payment].
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

    /// File containing the raw payment private key (EVM hex or Solana base58);
    /// `-` reads stdin.
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

// Resolve purchase config and build the keyless context before I/O.
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

// Resolve the keyless session config; balance and drip do not sign payments.
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

// Resolve the gateway path slug for a credit purchase.
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

// Load the payment defaults, treating a missing file as empty.
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

    // Confirm before the gateway probe because the purchase is single-attempt.
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
        drawdown_call_hint(&args, &payment, ctx.out.color)
    ));
    if matches!(ctx.global.format, Some(f) if f.is_structured()) || !ctx.out.stdout_is_tty {
        return emit_balance(&ctx, &balance);
    }
    Ok(())
}

// Build the next drawdown command. Credits are not network-scoped.
fn drawdown_call_hint(args: &PaymentArgs, payment: &PaymentConfig, color: bool) -> String {
    let (method, query_network) = if payment.pay_network.starts_with("solana:") {
        ("getSlot", "solana-mainnet")
    } else {
        ("eth_blockNumber", EXAMPLE_QUERY_NETWORK)
    };
    let mut cmd = format!(
        "  qn rpc call eth_blockNumber \\\n    \
           --network {EXAMPLE_QUERY_NETWORK} \\\n    \
           --x402-drawdown \\\n    \
           --payment-wallet {}",
        args.payment_wallet.as_deref().unwrap_or("<NAME>")
    );
    cmd = cmd
        .replace("eth_blockNumber", method)
        .replace(EXAMPLE_QUERY_NETWORK, query_network);
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
    reject_unsupported_drip(&args, &global)?;
    let ctx = session_setup(&args, global.clone())?;
    let session = ensure_gateway_session(&ctx, &global).await?;
    let receipt = ctx.sdk.rpc.gateway_drip(&session).await?;

    let settlement = receipt
        .transfer_id
        .as_deref()
        .map(|id| format!("transfer: {id}"))
        .or_else(|| {
            receipt
                .transaction_hash
                .as_deref()
                .map(|hash| format!("tx: {hash}"))
        })
        .unwrap_or_else(|| "settlement pending".to_string());
    ctx.out.note(&format!(
        "✓ Faucet request accepted for {} ({settlement})",
        receipt.account_id
    ));
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
            "wallet_address": receipt.wallet_address,
            "network": receipt.network,
            "transfer_id": receipt.transfer_id,
            "amount_usdc": receipt.amount_usdc,
            "transaction_hash": receipt.transaction_hash,
        });
        return super::emit_result(&ctx, &v);
    }
    if !ctx.out.stdout_is_tty {
        println!("{settlement}");
    }
    Ok(())
}

fn reject_unsupported_drip(args: &SessionArgs, global: &GlobalArgs) -> Result<(), CliError> {
    let section = load_payment_section(global)?;
    let Some(network) = args
        .payment_network
        .clone()
        .or_else(|| section.payment_network.clone())
    else {
        return Ok(());
    };
    let resolved = super::pay_network::resolve(&network)?;
    if !matches!(resolved.as_str(), "eip155:84532" | "eip155:5042002") {
        return Err(CliError::Arg(
            "x402 drip is available on Base Sepolia and Arc Testnet. Fund other wallets out of band."
                .to_string(),
        ));
    }
    Ok(())
}

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

fn fmt_credits(n: u64) -> String {
    group_digits(&n.to_string())
}

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
