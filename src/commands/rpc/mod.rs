//! RPC call and network-list commands. Default, custom-URL, and paid calls use
//! separate lanes so their auth, cache, and retry behavior cannot mix.

mod mpp;
mod pay_asset;
mod pay_network;
mod payment;
mod supported_networks;
mod x402;

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{ArgGroup, Args as ClapArgs, Subcommand};
use serde_json::Value;

use crate::confirm::{decide_without_prompt, ConfirmCfg, Severity};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::Format;
use crate::retry::retrying;
use crate::{config, confirm};

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: RpcCmd,
}

#[derive(Debug, Subcommand)]
pub enum RpcCmd {
    /// Make a JSON-RPC call.
    #[command(after_help = "Examples:\n  \
        qn rpc call eth_blockNumber\n  \
        qn rpc call eth_getBalance '[\"0xabc\", \"latest\"]'\n  \
        qn rpc call getSlot --network solana-mainnet\n  \
        qn rpc call eth_blockNumber --endpoint-url https://my-endpoint.example/rpc\n  \
        qn rpc call eth_call --params-file params.json\n  \
        echo '[{\"to\":\"0xabc\",\"data\":\"0x\"},\"latest\"]' | qn rpc call eth_call -\n  \
        cat params.json | qn rpc call eth_call -f -\n\n\
        Paid (crypto micropayment, no API key;\n  \
        the payment chain is independent of the chain you query):\n  \
        qn rpc call eth_blockNumber --network ethereum-mainnet --x402 \\\n      \
        --payment-wallet payer --payment-network base-sepolia \\\n      \
        --payment-asset USDC --max-amount 10000\n  \
        qn rpc call eth_blockNumber --network tempo-testnet --mpp --receipt \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset USDC --max-amount 10000\n  \
        qn rpc call getSlot --network solana-devnet --x402 \\\n      \
        --payment-wallet sol-payer --payment-network solana-devnet \\\n      \
        --payment-asset USDC --max-amount 1000000\n  \
        qn rpc call getSlot --network solana-devnet --x402 \\\n      \
        --payment-wallet sol-payer --payment-network solana-devnet \\\n      \
        --payment-asset USDC --max-amount 1000000 \\\n      \
        --svm-rpc-url https://my-solana-endpoint.example\n\n\
        Prepaid x402 credits (drawdown — buy once, then spend on any supported network):\n  \
        qn rpc x402 buy-credits --network ethereum-mainnet --payment-wallet payer \\\n      \
        --payment-network base-sepolia --payment-asset USDC --max-amount 10000000\n  \
         qn rpc call eth_blockNumber --network ethereum-mainnet --x402-drawdown --payment-wallet payer\n\n\
         qn rpc x402 buy-credits --network solana-devnet --payment-wallet sol-payer \\\n     \
         --payment-network solana-devnet --payment-asset 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU \\\n     \
         --max-amount 1000000\n\
         qn rpc call getSlot --network solana-devnet --x402-drawdown \\\n     \
         --payment-wallet sol-payer --payment-network solana-devnet\n\n\
         MPP payment channel (open once, then pay per call with a voucher):\n  \
        qn rpc mpp open --deposit 1000000 \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset USDC --max-amount 1000000\n  \
        qn rpc call eth_blockNumber --network ethereum-mainnet --mpp-session \\\n      \
        --payment-wallet payer --payment-network tempo-testnet \\\n      \
        --payment-asset USDC --max-amount 1000000\n\n\
        Discover networks and payment options, and manage wallets:\n  \
        qn rpc x402 supported-networks    (mpp: qn rpc mpp supported-networks)\n  \
        qn rpc x402 supported-payments    (mpp: qn rpc mpp supported-payments)\n  \
        qn wallet generate --vm evm --name payer          (x402/EVM and MPP)\n  \
        qn wallet generate --vm svm --name sol-payer      (x402/Solana)")]
    Call(Box<CallArgs>),

    /// List the endpoint's available network keys.
    #[command(visible_alias = "ls")]
    ListNetworks,

    /// Manage x402 credit drawdown: buy prepaid credits, check the balance, or
    /// drip testnet credits. Pair with `qn rpc call --x402-drawdown`.
    X402(x402::Args),

    /// Manage an MPP payment channel: open, top-up, close, or check status.
    /// Pair with `qn rpc call --mpp-session`.
    Mpp(mpp::Args),
}

#[derive(Debug, ClapArgs)]
#[command(
    group(ArgGroup::new("params_source").args(["params", "params_file"])),
    group(ArgGroup::new("payment").args(["x402", "mpp", "x402_drawdown", "mpp_session"])),
)]
pub struct CallArgs {
    /// The JSON-RPC method, e.g. `eth_blockNumber`.
    #[arg(value_name = "METHOD")]
    pub method: String,

    /// JSON params: an array (positional) or object (by-name). Pass `-` to read
    /// the JSON from stdin. Omit for no params (sends `[]`). Mutually exclusive
    /// with --params-file.
    ///
    /// To auto-enable Tooling Access when it isn't provisioned yet, pass the
    /// global `--yes`/`-y` flag (required in non-interactive contexts).
    #[arg(value_name = "PARAMS")]
    pub params: Option<String>,

    /// Read JSON params from a file, or from stdin when the path is `-`.
    /// Mutually exclusive with the positional params argument.
    #[arg(long, short = 'f', value_name = "PATH")]
    pub params_file: Option<PathBuf>,

    /// Target network on the multichain endpoint, by its key (e.g.
    /// `solana-mainnet`, `polygon`, `btc`). Omit for the endpoint's default
    /// network. Run `qn rpc list-networks` to see available keys.
    #[arg(long)]
    pub network: Option<String>,

    /// Send the call to a fully-formed custom HTTP URL instead of the account's
    /// Tooling Access endpoint. The URL is self-authenticating: no session token
    /// is minted or attached. Overrides `[rpc] endpoint_url` in config. Mutually
    /// exclusive with `--network` (a custom URL is not multichain-routed).
    #[arg(long, conflicts_with = "network", value_name = "URL")]
    pub endpoint_url: Option<String>,

    /// Pay for this call per request with the x402 protocol (EVM or Solana
    /// stablecoin) instead of the account's API key. Moves real funds; use a
    /// dedicated, minimally funded wallet. Requires --network (the query
    /// chain, as the payment gateway's path slug — e.g. `base-sepolia`).
    #[arg(long, conflicts_with = "endpoint_url", help_heading = "Payment")]
    pub x402: bool,

    /// Pay for this call per request with MPP (Tempo). Mutually exclusive
    /// with --x402; same rules otherwise.
    #[arg(long, conflicts_with = "endpoint_url", help_heading = "Payment")]
    pub mpp: bool,

    /// Pay for this call from prepaid x402 credits (drawdown): no per-call
    /// signing, 1 credit per successful response. Buy credits first with
    /// `qn rpc x402 buy-credits`. Requires --network (the query chain). The
    /// session JWT is authenticated and refreshed automatically.
    #[arg(long, conflicts_with = "endpoint_url", help_heading = "Payment")]
    pub x402_drawdown: bool,

    /// Pay for this call from an open MPP channel (session): a cumulative
    /// EIP-712 voucher, no on-chain tx per call. Open a channel first with
    /// `qn rpc mpp open`. Requires --network (the query chain).
    #[arg(long, conflicts_with = "endpoint_url", help_heading = "Payment")]
    pub mpp_session: bool,

    /// File containing the raw payment private key (EVM/Tempo hex, Solana
    /// base58); pass `-` to read it from stdin. Never accepts the key itself.
    /// Precedence: this flag > --payment-wallet > `key_file` > `wallet` under
    /// [rpc.payment] in config.
    #[arg(
        long,
        value_name = "PATH",
        requires = "payment",
        conflicts_with = "payment_wallet",
        help_heading = "Payment"
    )]
    pub payment_key_file: Option<PathBuf>,

    /// Name of a stored wallet (from `qn wallet generate`) to pay with. Its
    /// key file under `<config-dir>/qn/wallets/` is used. Mutually exclusive
    /// with --payment-key-file.
    #[arg(
        long,
        value_name = "NAME",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub payment_wallet: Option<String>,

    /// Spend ceiling per call, in integer base units of the asset (e.g.
    /// 10000 = 0.01 USDC). No built-in default: flag > `max_amount` under
    /// [rpc.payment]. Offered payments above the ceiling are never signed;
    /// among those at or under it, the cheapest is paid.
    #[arg(
        long,
        value_name = "BASE_UNITS",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub max_amount: Option<String>,

    /// Chain you PAY on — a network name (e.g. `base-sepolia`) or CAIP-2 id
    /// (e.g. `eip155:84532`) — independent of --network (the chain you
    /// query). Falls back to `payment_network` under [rpc.payment].
    #[arg(
        long,
        value_name = "NETWORK",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub payment_network: Option<String>,

    /// Token to pay with: EVM contract address, Solana mint, or a symbol like
    /// USDC (resolved per network). Falls back to `payment_asset` under
    /// [rpc.payment].
    #[arg(
        long,
        value_name = "ADDRESS",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub payment_asset: Option<String>,

    /// Explicit Solana RPC URL for building x402/Solana payments. Falls back
    /// to `svm_rpc_url` under [rpc.payment], then a public Solana RPC (which
    /// rate-limits aggressively — set this at any real volume).
    #[arg(
        long,
        value_name = "URL",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub svm_rpc_url: Option<String>,

    /// Wrap stdout as {"result": ..., "payment_receipt": ...}. The receipt is
    /// non-null only on MPP (the settlement transaction hash); null on x402.
    /// Payment happens either way — this only changes the output.
    #[arg(long, requires = "payment", help_heading = "Payment")]
    pub receipt: bool,
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        RpcCmd::Call(call) => run_call(*call, global).await,
        RpcCmd::ListNetworks => run_list_networks(global).await,
        RpcCmd::X402(x402_args) => x402::run(x402_args, global).await,
        RpcCmd::Mpp(mpp_args) => mpp::run(mpp_args, global).await,
    }
}

/// List network keys from the Tooling Access endpoint.
async fn run_list_networks(global: GlobalArgs) -> Result<(), CliError> {
    let config_path = global.resolve_config_path();
    let networks_path = config::networks_cache_path(config_path.as_deref());
    let token_path = config::token_cache_path(config_path.as_deref());
    let (seed, _account_id) =
        load_cached_token(&token_path, resolve_key_quietly(&global).as_deref());
    let (ctx, _api_key) = Ctx::from_global_with_rpc_seed(global, seed, None)?;
    let map = ensure_networks(&ctx, networks_path.as_deref()).await?;
    emit_networks(&ctx, &map)
}

async fn run_call(args: CallArgs, global: GlobalArgs) -> Result<(), CliError> {
    // Paid lanes bypass Tooling Access caches and recovery.
    if args.x402 || args.mpp {
        return payment::run_paid_call(args, global).await;
    }
    if args.x402_drawdown {
        return payment::run_drawdown_call(args, global).await;
    }
    if args.mpp_session {
        return payment::run_session_call(args, global).await;
    }

    let params = parse_params(args.params.as_deref(), args.params_file.as_deref())?;

    // Custom URLs bypass JWT and Tooling Access recovery.
    let flag_endpoint_url = match args.endpoint_url.as_deref() {
        Some(u) => Some(crate::context::validate_endpoint_url(u)?),
        None => None,
    };
    let config_endpoint_url = load_config_endpoint_url(&global);
    let custom_url = flag_endpoint_url
        .clone()
        .or_else(|| config_endpoint_url.clone());

    let config_path = global.resolve_config_path();
    let token_path = config::token_cache_path(config_path.as_deref());
    let networks_path = config::networks_cache_path(config_path.as_deref());

    let (seed, mut account_id) =
        load_cached_token(&token_path, resolve_key_quietly(&global).as_deref());

    let (ctx, api_key) = Ctx::from_global_with_rpc_seed(global, seed, config_endpoint_url)?;

    if args.network.is_some() {
        let map = ensure_networks(&ctx, networks_path.as_deref()).await?;
        ctx.sdk.rpc.set_networks(map);
    }

    let method = args.method.as_str();

    let result = match call_once(
        &ctx,
        method,
        &params,
        args.network.clone(),
        custom_url.clone(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) if custom_url.is_some() => return Err(map_unknown_network(e)),
        Err(e) if is_not_enabled(&e) => {
            maybe_enable(&ctx).await?;
            call_after_enable(&ctx, method, &params, args.network.clone()).await?
        }
        Err(e) if is_transport_failure(&e) => {
            if disabled_per_status(&ctx).await {
                ctx.sdk.rpc.clear_cached_token();
                if let (Some(p), Some(id)) = (&token_path, account_id) {
                    let _ = config::delete_account_token(p, id);
                }
                maybe_enable(&ctx).await?;
                call_after_enable(&ctx, method, &params, args.network.clone()).await?
            } else {
                return Err(e);
            }
        }
        Err(e) => return Err(map_unknown_network(e)),
    };

    // Persist a refreshed token, but never let cache failure fail a successful call.
    if let (Some(p), Some(current)) = (&token_path, ctx.sdk.rpc.current_token()) {
        if account_id.is_none() {
            account_id = ctx
                .sdk
                .admin
                .account_info()
                .await
                .ok()
                .and_then(|r| r.data)
                .map(|a| a.id);
        }
        if let Some(id) = account_id {
            let _ = config::save_token(p, &api_key, id, &current);
        }
    }

    emit_result(&ctx, &result)
}

/// Reads `[rpc] endpoint_url` from the resolved config file, if any. Swallows
/// load errors (a broken config surfaces later when `Ctx` builds); returns the
/// raw value, which `Ctx::build` validates.
fn load_config_endpoint_url(global: &GlobalArgs) -> Option<String> {
    let path = global.resolve_config_path()?;
    match config::load_from(&path) {
        Ok(Some(cfg)) => cfg.rpc.endpoint_url,
        _ => None,
    }
}

/// Re-render the SDK's "unknown network" config error with CLI wording that
/// points at `qn rpc list-networks`, dropping the SDK-internal
/// `set_networks()` hint. Any other error passes through unchanged.
fn map_unknown_network(err: CliError) -> CliError {
    if let CliError::Sdk(quicknode_sdk::errors::SdkError::Config(msg)) = &err {
        if msg.contains("unknown network") {
            // Keep the "Available: ..." list the SDK computed; replace only the
            // SDK-internal seeding hint with the CLI discovery command.
            let available = msg
                .split_once("Available:")
                .map(|(_, rest)| rest.trim())
                .filter(|s| !s.is_empty());
            let mut out = "unknown network key for this endpoint.".to_string();
            if let Some(list) = available {
                out.push_str(&format!(" Available: {list}"));
            }
            out.push_str("\nRun 'qn rpc list-networks' to see valid keys.");
            return CliError::Arg(out);
        }
    }
    err
}

/// Current unix time in seconds, for the networks-cache TTL.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Load the endpoint's per-network URL map from cache or the API.
async fn ensure_networks(
    ctx: &Ctx,
    networks_path: Option<&std::path::Path>,
) -> Result<std::collections::HashMap<String, String>, CliError> {
    let mut status = retrying(ctx.global.retries, || ctx.sdk.admin.tooling_access_status()).await?;

    if !status.enabled {
        maybe_enable(ctx).await?;
        status = retrying(ctx.global.retries, || ctx.sdk.admin.tooling_access_status()).await?;
    }

    let Some(endpoint_id) = status.endpoint_id else {
        return Err(CliError::Arg(
            "this account's Tooling Access endpoint did not report an id, so per-network \
             routing is unavailable. Omit --network to use the default network."
                .to_string(),
        ));
    };

    if let Some(p) = networks_path {
        if let Some(map) = config::load_networks(p, &endpoint_id, now_unix()) {
            return Ok(map);
        }
    }

    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_endpoint_urls(&endpoint_id)
    })
    .await?;
    let map: std::collections::HashMap<String, String> = resp
        .data
        .and_then(|d| d.multichain_urls)
        .map(|mc| mc.into_iter().map(|(k, v)| (k, v.http_url)).collect())
        .unwrap_or_default();
    if let Some(p) = networks_path {
        let _ = config::save_networks(p, &endpoint_id, now_unix(), &map);
    }
    Ok(map)
}

/// Print the available network keys, one per line (or as JSON/yaml/toon).
fn emit_networks(
    ctx: &Ctx,
    map: &std::collections::HashMap<String, String>,
) -> Result<(), CliError> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let v = serde_json::json!({ "networks": keys });
        return emit_result(ctx, &v);
    }
    for k in keys {
        println!("{k}");
    }
    Ok(())
}

/// Load a cached token and account ID without network access.
fn load_cached_token(
    token_path: &Option<PathBuf>,
    key: Option<&str>,
) -> (Option<quicknode_sdk::CachedToken>, Option<i64>) {
    let (Some(p), Some(key)) = (token_path, key) else {
        return (None, None);
    };
    let Some(account_id) = config::account_for_key(p, key) else {
        return (None, None);
    };
    (
        config::load_token_for_account(p, account_id),
        Some(account_id),
    )
}

/// Resolve the API key quietly for cache lookup.
fn resolve_key_quietly(global: &GlobalArgs) -> Option<String> {
    let path = global.resolve_config_path();
    config::resolve_api_key(global.api_key.as_deref(), path.as_deref(), false, || {
        Err(CliError::NoApiKey)
    })
    .ok()
    .map(|(k, _)| k)
}

async fn call_once(
    ctx: &Ctx,
    method: &str,
    params: &Option<Value>,
    network: Option<String>,
    endpoint_url: Option<String>,
) -> Result<Value, CliError> {
    retrying(ctx.global.retries, || {
        ctx.sdk.rpc.call(
            method,
            params.clone(),
            network.clone(),
            endpoint_url.clone(),
        )
    })
    .await
    .map_err(Into::into)
}

// Retry budget while a newly enabled endpoint becomes routable.
const POST_ENABLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
const POST_ENABLE_INITIAL_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Retry transient failures while a new endpoint provisions.
async fn call_after_enable(
    ctx: &Ctx,
    method: &str,
    params: &Option<Value>,
    network: Option<String>,
) -> Result<Value, CliError> {
    tokio::time::sleep(POST_ENABLE_INITIAL_WAIT).await;

    let deadline = tokio::time::Instant::now() + POST_ENABLE_BUDGET;
    let mut backoff = std::time::Duration::from_millis(500);
    loop {
        match ctx
            .sdk
            .rpc
            .call(method, params.clone(), network.clone(), None)
            .await
        {
            Ok(v) => return Ok(v),
            Err(e) => {
                let cli_err = CliError::from(e);
                let now = tokio::time::Instant::now();
                if !is_transport_failure(&cli_err) || now >= deadline {
                    return Err(cli_err);
                }
                let remaining = deadline - now;
                tokio::time::sleep(backoff.min(remaining)).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(4));
            }
        }
    }
}

/// Check for the control plane's "not enabled" response.
fn is_not_enabled(err: &CliError) -> bool {
    matches!(
        err,
        CliError::Sdk(quicknode_sdk::errors::SdkError::Api { status, body })
            if status.as_u16() == 400 && body.to_lowercase().contains("not enabled")
    )
}

/// Check for a connect or timeout failure.
fn is_transport_failure(err: &CliError) -> bool {
    use quicknode_sdk::errors::HttpKind;
    matches!(
        err,
        CliError::Sdk(sdk @ quicknode_sdk::errors::SdkError::Http(_))
            if matches!(sdk.http_kind(), Some(HttpKind::Connect | HttpKind::Timeout))
    )
}

/// Check whether a failed RPC targets a disabled endpoint.
async fn disabled_per_status(ctx: &Ctx) -> bool {
    matches!(ctx.sdk.admin.tooling_access_status().await, Ok(s) if !s.enabled)
}

/// Enable Tooling Access with the standard confirmation behavior.
async fn maybe_enable(ctx: &Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );

    let proceed = match decide_without_prompt(Severity::Mild, cfg) {
        Ok(p) => p,
        Err(CliError::NeedsConfirmation) => {
            return Err(CliError::Arg(
                "Tooling Access is not enabled for this account. \
                 Run 'qn tooling-access enable', or pass --yes to enable it now."
                    .to_string(),
            ));
        }
        Err(e) => return Err(e),
    };

    let proceed =
        proceed || confirm::prompt_yes_no("Tooling Access is not enabled. Enable it now?")?;
    if !proceed {
        return Err(CliError::Cancelled);
    }

    ctx.sdk.admin.enable_tooling_access().await?;
    ctx.out.note("✓ Enabled Tooling Access");
    Ok(())
}

/// Read and parse inline or file-based JSON-RPC params.
pub(super) fn parse_params(
    arg: Option<&str>,
    file: Option<&Path>,
) -> Result<Option<Value>, CliError> {
    let raw = match (arg, file) {
        (None, None) => return Ok(None),
        (Some("-"), _) => read_stdin("params")?,
        (Some(s), _) => s.to_string(),
        (None, Some(path)) if path.as_os_str() == "-" => read_stdin("params")?,
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| {
            CliError::Arg(format!(
                "could not read params file '{}': {e}",
                path.display()
            ))
        })?,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| CliError::Arg(format!("params is not valid JSON: {e}")))?;
    Ok(Some(value))
}

/// Read all of stdin as a UTF-8 string, labeling errors with `what`.
pub(super) fn read_stdin(what: &str) -> Result<String, CliError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| CliError::Arg(format!("could not read {what} from stdin: {e}")))?;
    Ok(buf)
}

/// Emit a schemaless RPC result in the requested format.
pub(super) fn emit_result(ctx: &Ctx, result: &Value) -> Result<(), CliError> {
    match ctx.global.format {
        None | Some(Format::Json) => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).map_err(CliError::Json)?
            );
        }
        Some(Format::Yaml) => {
            print!(
                "{}",
                serde_yml::to_string(result).map_err(|e| CliError::Format(e.to_string()))?
            );
        }
        Some(Format::Toon) => {
            println!(
                "{}",
                toon_format::encode_default(result).map_err(|e| CliError::Format(e.to_string()))?
            );
        }
        Some(fmt @ (Format::Table | Format::Md)) => {
            let name = if fmt == Format::Md { "md" } else { "table" };
            ctx.out.warn(&format!(
                "ℹ '-o {name}' has no columns for 'qn rpc'; printing JSON. Use -o json/yaml/toon for structured output."
            ));
            println!(
                "{}",
                serde_json::to_string_pretty(result).map_err(CliError::Json)?
            );
        }
    }
    Ok(())
}
