//! Keyless x402/MPP payment lane. Configuration is resolved before network I/O;
//! paid calls are single-attempt because a lost response may mean settlement.

use std::path::Path;

use quicknode_sdk::errors::SdkError;
use quicknode_sdk::{GatewaySession, PaymentConfig};

use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::{style, Style};

use super::CallArgs;

// Refresh cached sessions before expiry to absorb clock skew.
const SESSION_MARGIN_SECS: i64 = 60;

/// Reject using stdin for both params and the payment key.
fn check_single_stdin(args: &CallArgs) -> Result<(), CliError> {
    let params_use_stdin = matches!(args.params.as_deref(), Some("-"))
        || matches!(&args.params_file, Some(p) if p.as_os_str() == "-");
    let key_use_stdin = matches!(&args.payment_key_file, Some(p) if p.as_os_str() == "-");
    if params_use_stdin && key_use_stdin {
        return Err(CliError::Arg(
            "cannot read both the params and the payment key from stdin; \
             put one of them in a file"
                .to_string(),
        ));
    }
    Ok(())
}

pub(super) async fn run_paid_call(args: CallArgs, global: GlobalArgs) -> Result<(), CliError> {
    let params_use_stdin = matches!(args.params.as_deref(), Some("-"))
        || matches!(&args.params_file, Some(p) if p.as_os_str() == "-");
    let key_use_stdin = matches!(&args.payment_key_file, Some(p) if p.as_os_str() == "-");
    if params_use_stdin && key_use_stdin {
        return Err(CliError::Arg(
            "cannot read both the params and the payment key from stdin; \
             put one of them in a file"
                .to_string(),
        ));
    }

    let section = load_payment_section(&global)?;

    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());

    let (payment, network, key_file_warning) = resolve_payment_config(
        &args,
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;

    let params = super::parse_params(args.params.as_deref(), args.params_file.as_deref())?;

    let ctx = Ctx::from_global_keyless_payment(global, payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }

    let resp = ctx
        .sdk
        .rpc
        .call_with_receipt(&args.method, params, Some(network), None)
        .await
        .map_err(map_paid_error)?;

    if args.receipt {
        let receipt = resp.payment_receipt.map(|r| {
            serde_json::json!({
                "method": r.method,
                "status": r.status,
                "timestamp": r.timestamp,
                "reference": r.reference,
            })
        });
        super::emit_result(
            &ctx,
            &serde_json::json!({
                "result": resp.result,
                "payment_receipt": receipt,
            }),
        )
    } else {
        super::emit_result(&ctx, &resp.result)
    }
}

// Drawdown calls use a cached Bearer session and one transparent auth refresh.
pub(super) async fn run_drawdown_call(args: CallArgs, global: GlobalArgs) -> Result<(), CliError> {
    check_single_stdin(&args)?;
    let Some(network) = args.network.clone() else {
        return Err(CliError::Arg(
            "--x402-drawdown requires --network: the chain the call queries, as \
             the payment gateway's path slug (e.g. --network base-sepolia)."
                .to_string(),
        ));
    };

    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_drawdown_config(
        &args,
        &network,
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;

    let params = super::parse_params(args.params.as_deref(), args.params_file.as_deref())?;

    let ctx = Ctx::from_global_keyless_payment(global.clone(), payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }

    let session = ensure_gateway_session(&ctx, &global).await?;

    let result = match ctx
        .sdk
        .rpc
        .gateway_drawdown_call(&args.method, params.clone(), &network, &session)
        .await
    {
        Ok(v) => v,
        Err(e) if is_token_expired(&e) => {
            let fresh = reauthenticate(&ctx, &global).await?;
            match ctx
                .sdk
                .rpc
                .gateway_drawdown_call(&args.method, params, &network, &fresh)
                .await
            {
                Ok(v) => v,
                Err(e) => return Err(drawdown_failure(&ctx, &args, &network, e)),
            }
        }
        Err(e) => return Err(drawdown_failure(&ctx, &args, &network, e)),
    };

    super::emit_result(&ctx, &result)
}

/// Load a fresh cached gateway session or authenticate one.
pub(super) async fn ensure_gateway_session(
    ctx: &Ctx,
    global: &GlobalArgs,
) -> Result<GatewaySession, CliError> {
    let sessions_path = config::sessions_cache_path(global.resolve_config_path().as_deref());
    let address = ctx.sdk.rpc.payment_address()?;

    if let Some(path) = &sessions_path {
        if let Some(existing) = config::load_gateway_session_by_address(path, &address) {
            if existing.is_fresh(SESSION_MARGIN_SECS) {
                return Ok(existing);
            }
        }
    }
    reauthenticate(ctx, global).await
}

/// Authenticate and replace the cached session.
async fn reauthenticate(ctx: &Ctx, global: &GlobalArgs) -> Result<GatewaySession, CliError> {
    let sessions_path = config::sessions_cache_path(global.resolve_config_path().as_deref());
    let address = ctx.sdk.rpc.payment_address()?;
    let session = ctx.sdk.rpc.gateway_authenticate().await?;
    if let Some(path) = &sessions_path {
        let _ = config::save_gateway_session(path, &address, &session);
    }
    Ok(session)
}

/// Check for an expired gateway session.
fn is_token_expired(e: &SdkError) -> bool {
    matches!(
        e,
        SdkError::Api { status, body }
            if matches!(status.as_u16(), 401 | 403)
                && (body.contains("token_expired") || body.contains("invalid_token"))
    )
}

fn is_out_of_credits(e: &SdkError) -> bool {
    matches!(
        e,
        SdkError::Api { status, body }
            if status.as_u16() == 402
                || body.contains("insufficient_credits")
                || body.contains("no_credits")
    )
}

// Add a balance hint for empty-credit failures, then map the error.
fn drawdown_failure(ctx: &Ctx, args: &CallArgs, network: &str, e: SdkError) -> CliError {
    if is_out_of_credits(&e) {
        let wallet = args.payment_wallet.as_deref().unwrap_or("<NAME>");
        let pay_net = args.payment_network.as_deref().unwrap_or(network);
        ctx.out.note(&format!(
            "{}\n\n{}\n",
            style("Check balance:", Style::Bold, ctx.out.color),
            style(
                &format!(
                    "  qn rpc x402 balance \\\n    \
                       --payment-wallet {wallet} \\\n    \
                       --payment-network {pay_net}"
                ),
                Style::Bold,
                ctx.out.color,
            ),
        ));
    }
    map_drawdown_error(e)
}

fn map_drawdown_error(e: SdkError) -> CliError {
    if is_out_of_credits(&e) {
        return CliError::PaymentRefused(
            "out of x402 credits. Buy more with 'qn rpc x402 buy-credits', \
             then retry this call."
                .to_string(),
        );
    }
    if let SdkError::Api { body, .. } = &e {
        if body.contains("monthly_limit_reached") {
            return CliError::PaymentRefused(
                "the account's monthly x402 limit was reached; no credits were \
                 drawn. Try again after the limit resets."
                    .to_string(),
            );
        }
    }
    e.into()
}

// Pay from a cached MPP channel with one cumulative voucher.
pub(super) async fn run_session_call(args: CallArgs, global: GlobalArgs) -> Result<(), CliError> {
    check_single_stdin(&args)?;
    let Some(network) = args.network.clone() else {
        return Err(CliError::Arg(
            "--mpp-session requires --network: the chain the call queries, as \
             the payment gateway's path slug (e.g. --network tempo-testnet)."
                .to_string(),
        ));
    };

    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_payment_params(
        "mpp",
        &args.payment_params(),
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;
    let params = super::parse_params(args.params.as_deref(), args.params_file.as_deref())?;

    let pay_scope = super::mpp::PayScope::from_config(&payment);
    let ctx = Ctx::from_global_keyless_payment(global.clone(), payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }

    let address = ctx.sdk.rpc.payment_address()?;
    let scope = pay_scope.with_address(address);
    let channels_path = config::channels_cache_path(global.resolve_config_path().as_deref());
    let mut channel = channels_path
        .as_deref()
        .and_then(|p| config::load_channel(p, &scope))
        .ok_or_else(|| {
            CliError::Arg(format!(
                "no open MPP channel for this wallet paying {}. Open one with \
                 'qn rpc mpp open --deposit <BASE_UNITS>'.",
                pay_scope.describe()
            ))
        })?;

    let new_cumulative = channel.cumulative_spent.saturating_add(channel.per_call);
    if new_cumulative > channel.deposit {
        return Err(CliError::Arg(format!(
            "MPP channel deposit exhausted (deposit {}, would need {}). Top up \
             with 'qn rpc mpp top-up --deposit <BASE_UNITS>'.",
            channel.deposit, new_cumulative
        )));
    }

    let result = ctx
        .sdk
        .rpc
        .mpp_session_call(&args.method, params, &network, &channel, new_cumulative)
        .await
        .map_err(map_session_error)?;

    channel.cumulative_spent = new_cumulative;
    if let Some(path) = &channels_path {
        let _ = config::save_channel(path, &scope, &channel);
    }

    super::emit_result(&ctx, &result)
}

fn map_session_error(e: SdkError) -> CliError {
    if let SdkError::Api { status, body } = &e {
        if status.as_u16() == 402
            || body.contains("amount-exceeds-deposit")
            || body.contains("AmountExceedsDeposit")
            || body.contains("insufficient")
        {
            return CliError::PaymentRefused(
                "the MPP channel can't cover this call. Top up with \
                 'qn rpc mpp top-up', or open a new channel with 'qn rpc mpp open'."
                    .to_string(),
            );
        }
    }
    map_paid_error(e)
}

fn load_payment_section(global: &GlobalArgs) -> Result<PaymentSection, CliError> {
    let Some(path) = global.resolve_config_path() else {
        return Ok(PaymentSection::default());
    };
    Ok(config::load_from(&path)?
        .map(|cfg| cfg.rpc.payment)
        .unwrap_or_default())
}

/// Payment parameters shared by paid call and lifecycle commands.
pub(super) struct PaymentParams<'a> {
    pub key_file: Option<&'a Path>,
    pub wallet: Option<&'a str>,
    pub max_amount: Option<&'a str>,
    pub payment_network: Option<&'a str>,
    pub payment_asset: Option<&'a str>,
    pub svm_rpc_url: Option<&'a str>,
}

/// Parameters needed by keyless session commands.
pub(super) struct SessionParams<'a> {
    pub key_file: Option<&'a Path>,
    pub wallet: Option<&'a str>,
    pub payment_network: Option<&'a str>,
    pub svm_rpc_url: Option<&'a str>,
}

impl CallArgs {
    fn payment_params(&self) -> PaymentParams<'_> {
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

fn resolve_payment_config(
    args: &CallArgs,
    section: &PaymentSection,
    wallets_dir: Option<&Path>,
    base_url_override: Option<String>,
) -> Result<(PaymentConfig, String, Option<String>), CliError> {
    let scheme = if args.x402 { "x402" } else { "mpp" };

    let Some(network) = args.network.clone() else {
        return Err(CliError::Arg(format!(
            "--{scheme} requires --network: the chain the call queries, as the \
             payment gateway's path slug (e.g. --network base-sepolia). This is \
             separate from --payment-network, the chain the payment settles on."
        )));
    };

    let payment = resolve_payment_params(
        scheme,
        &args.payment_params(),
        section,
        wallets_dir,
        base_url_override,
    )?;
    let key_file_warning = payment.1;
    Ok((payment.0, network, key_file_warning))
}

// Resolve the wallet and pay network for a drawdown session.
fn resolve_drawdown_config(
    args: &CallArgs,
    query_network: &str,
    section: &PaymentSection,
    wallets_dir: Option<&Path>,
    base_url_override: Option<String>,
) -> Result<(PaymentConfig, Option<String>), CliError> {
    if section.key.is_some() {
        return Err(CliError::Arg(
            "[rpc.payment] does not accept an inline `key`; store the key in a \
             file and set `key_file = \"<path>\"` instead"
                .to_string(),
        ));
    }
    let (key, key_file_warning) = resolve_key(
        args.payment_key_file.as_deref(),
        args.payment_wallet.as_deref(),
        section.key_file.as_deref(),
        section.wallet.as_deref(),
        wallets_dir,
    )?;

    let payment_network = args
        .payment_network
        .clone()
        .or_else(|| section.payment_network.clone())
        .unwrap_or_else(|| query_network.to_string());
    let payment_network = super::pay_network::resolve(&payment_network)?;

    let svm_rpc_url = match args
        .svm_rpc_url
        .clone()
        .or_else(|| section.svm_rpc_url.clone())
    {
        Some(u) => Some(crate::context::validate_endpoint_url(&u)?),
        None => None,
    };

    Ok((
        PaymentConfig {
            scheme: "x402".to_string(),
            key,
            pay_network: payment_network,
            asset: String::new(),
            max_amount: "0".to_string(),
            svm_rpc_url,
            base_url_override,
        },
        key_file_warning,
    ))
}

// Resolve the wallet and pay network for balance/drip.
pub(super) fn resolve_session_params(
    params: &SessionParams<'_>,
    section: &PaymentSection,
    wallets_dir: Option<&Path>,
    base_url_override: Option<String>,
) -> Result<(PaymentConfig, Option<String>), CliError> {
    if section.key.is_some() {
        return Err(CliError::Arg(
            "[rpc.payment] does not accept an inline `key`; store the key in a \
             file and set `key_file = \"<path>\"` instead"
                .to_string(),
        ));
    }

    let (key, key_file_warning) = resolve_key(
        params.key_file,
        params.wallet,
        section.key_file.as_deref(),
        section.wallet.as_deref(),
        wallets_dir,
    )?;

    let payment_network = params
        .payment_network
        .map(str::to_string)
        .or_else(|| section.payment_network.clone())
        .ok_or_else(|| {
            CliError::Arg(
                "no payment network set. Pass --payment-network <NETWORK> (a \
                 network name like base-sepolia, or a CAIP-2 id like \
                 eip155:84532) or set `payment_network` under [rpc.payment]"
                    .to_string(),
            )
        })?;
    let payment_network = super::pay_network::resolve(&payment_network)?;

    let svm_rpc_url = match params
        .svm_rpc_url
        .map(str::to_string)
        .or_else(|| section.svm_rpc_url.clone())
    {
        Some(u) => Some(crate::context::validate_endpoint_url(&u)?),
        None => None,
    };

    Ok((
        PaymentConfig {
            scheme: "x402".to_string(),
            key,
            pay_network: payment_network,
            asset: String::new(),
            max_amount: "0".to_string(),
            svm_rpc_url,
            base_url_override,
        },
        key_file_warning,
    ))
}

/// Resolve payment parameters before any network I/O.
pub(super) fn resolve_payment_params(
    scheme: &str,
    params: &PaymentParams<'_>,
    section: &PaymentSection,
    wallets_dir: Option<&Path>,
    base_url_override: Option<String>,
) -> Result<(PaymentConfig, Option<String>), CliError> {
    if section.key.is_some() {
        return Err(CliError::Arg(
            "[rpc.payment] does not accept an inline `key`; store the key in a \
             file and set `key_file = \"<path>\"` instead (the config file is \
             too easily shared to hold a raw wallet key)"
                .to_string(),
        ));
    }

    let (key, key_file_warning) = resolve_key(
        params.key_file,
        params.wallet,
        section.key_file.as_deref(),
        section.wallet.as_deref(),
        wallets_dir,
    )?;

    let max_amount = params
        .max_amount
        .map(str::to_string)
        .or_else(|| section.max_amount.clone())
        .ok_or_else(|| {
            CliError::Arg(
                "no spend ceiling set. Pass --max-amount <BASE_UNITS> or set \
                 `max_amount` under [rpc.payment]. This is the most a single \
                 call may pay, in integer base units of the asset (e.g. \
                 10000 = 0.01 USDC)"
                    .to_string(),
            )
        })?;
    if max_amount.parse::<u128>().is_err() {
        return Err(CliError::Arg(format!(
            "--max-amount must be a non-negative integer in the asset's base \
             units (e.g. 10000 = 0.01 USDC), got '{max_amount}'"
        )));
    }

    let payment_network = params
        .payment_network
        .map(str::to_string)
        .or_else(|| section.payment_network.clone())
        .ok_or_else(|| {
            CliError::Arg(
                "no payment network set. Pass --payment-network <NETWORK> (a \
                 network name like base-sepolia, or a CAIP-2 id like \
                 eip155:84532) or set `payment_network` under [rpc.payment]"
                    .to_string(),
            )
        })?;
    let payment_network = super::pay_network::resolve(&payment_network)?;

    let payment_asset = params
        .payment_asset
        .map(str::to_string)
        .or_else(|| section.payment_asset.clone())
        .ok_or_else(|| {
            CliError::Arg(
                "no payment asset set. Pass --payment-asset <ADDRESS> (a token \
                 contract or mint to pay with, or a symbol like USDC) or set \
                 `payment_asset` under [rpc.payment]"
                    .to_string(),
            )
        })?;
    let payment_asset = super::pay_asset::resolve(&payment_asset, &payment_network)?;

    let svm_rpc_url = match params
        .svm_rpc_url
        .map(str::to_string)
        .or_else(|| section.svm_rpc_url.clone())
    {
        Some(u) => Some(crate::context::validate_endpoint_url(&u)?),
        None => None,
    };

    Ok((
        PaymentConfig {
            scheme: scheme.to_string(),
            key,
            pay_network: payment_network,
            asset: payment_asset,
            max_amount,
            svm_rpc_url,
            base_url_override,
        },
        key_file_warning,
    ))
}

/// Resolve a payment key from a file or stored wallet.
fn resolve_key(
    flag_file: Option<&Path>,
    flag_wallet: Option<&str>,
    config_file: Option<&Path>,
    config_wallet: Option<&str>,
    wallets_dir: Option<&Path>,
) -> Result<(String, Option<String>), CliError> {
    if let Some(path) = flag_file {
        if path.as_os_str() == "-" {
            let key = super::read_stdin("the payment key")?;
            return checked_key(key, "stdin").map(|k| (k, None));
        }
        return read_key_file(path);
    }
    if let Some(name) = flag_wallet {
        return read_key_file(&crate::commands::wallet::key_path(name, wallets_dir)?);
    }
    if let Some(path) = config_file {
        return read_key_file(path);
    }
    if let Some(name) = config_wallet {
        return read_key_file(&crate::commands::wallet::key_path(name, wallets_dir)?);
    }
    Err(CliError::Arg(
        "no payment key found. Pass --payment-key-file <PATH> (or '-' for \
         stdin), --payment-wallet <NAME> (from 'qn wallet generate'), or \
         set `key_file`/`wallet` under [rpc.payment]"
            .to_string(),
    ))
}

/// Read a key file and warn on loose Unix permissions.
fn read_key_file(path: &Path) -> Result<(String, Option<String>), CliError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        CliError::Arg(format!(
            "could not read payment key file '{}': {e}",
            path.display()
        ))
    })?;
    let key = checked_key(raw, &format!("'{}'", path.display()))?;

    #[cfg(unix)]
    let warning = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode())
            .filter(|mode| mode & 0o077 != 0)
            .map(|_| {
                format!(
                    "⚠ payment key file '{}' is readable by other users; \
                     consider `chmod 600`",
                    path.display()
                )
            })
    };
    #[cfg(not(unix))]
    let warning = None;

    Ok((key, warning))
}

/// Trim and reject an empty key without exposing its contents.
fn checked_key(raw: String, source: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::Arg(format!(
            "the payment key from {source} is empty"
        )));
    }
    Ok(trimmed.to_string())
}

/// Map post-payment decode and 5xx failures to the unknown-outcome bucket.
pub(super) fn map_paid_error(e: SdkError) -> CliError {
    match &e {
        SdkError::Decode { .. } => CliError::PaymentMaybeCharged(e),
        SdkError::PaymentRejected { status, .. } if *status >= 500 => {
            CliError::PaymentMaybeCharged(e)
        }
        _ => e.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paid_args(x402: bool) -> CallArgs {
        CallArgs {
            method: "eth_blockNumber".to_string(),
            params: None,
            params_file: None,
            network: Some("base-sepolia".to_string()),
            endpoint_url: None,
            x402,
            mpp: !x402,
            x402_drawdown: false,
            mpp_session: false,
            payment_key_file: None,
            payment_wallet: None,
            max_amount: Some("10000".to_string()),
            payment_network: Some("eip155:84532".to_string()),
            payment_asset: Some("0xabc".to_string()),
            svm_rpc_url: None,
            receipt: false,
        }
    }

    fn empty_section() -> PaymentSection {
        PaymentSection::default()
    }

    fn key_file_with(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn paid_args_with_key(x402: bool) -> (CallArgs, tempfile::NamedTempFile) {
        let f = key_file_with("0xkey\n");
        let mut args = paid_args(x402);
        args.payment_key_file = Some(f.path().to_path_buf());
        (args, f)
    }

    #[test]
    fn resolves_full_config_from_flags_and_key_file() {
        let (args, _f) = paid_args_with_key(true);
        let (cfg, network, _) =
            resolve_payment_config(&args, &empty_section(), None, None).unwrap();
        assert_eq!(cfg.scheme, "x402");
        assert_eq!(cfg.key, "0xkey");
        assert_eq!(cfg.pay_network, "eip155:84532");
        assert_eq!(cfg.asset, "0xabc");
        assert_eq!(cfg.max_amount, "10000");
        assert_eq!(network, "base-sepolia");
    }

    #[test]
    fn pay_network_name_resolves_to_caip2() {
        let (mut args, _f) = paid_args_with_key(true);
        args.payment_network = Some("base-sepolia".to_string());
        let (cfg, _, _) = resolve_payment_config(&args, &empty_section(), None, None).unwrap();
        assert_eq!(cfg.pay_network, "eip155:84532");
    }

    #[test]
    fn config_pay_network_name_resolves_too() {
        let mut section = empty_section();
        section.payment_network = Some("solana-devnet".to_string());
        let (mut args, _f) = paid_args_with_key(true);
        args.payment_network = None;
        let (cfg, _, _) = resolve_payment_config(&args, &section, None, None).unwrap();
        assert_eq!(cfg.pay_network, "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1");
    }

    #[test]
    fn unknown_pay_network_name_is_an_arg_error() {
        let (mut args, _f) = paid_args_with_key(true);
        args.payment_network = Some("btc".to_string());
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown pay network 'btc'"), "got: {msg}");
    }

    #[test]
    fn mpp_flag_selects_mpp_scheme() {
        let (args, _f) = paid_args_with_key(false);
        let (cfg, _, _) = resolve_payment_config(&args, &empty_section(), None, None).unwrap();
        assert_eq!(cfg.scheme, "mpp");
    }

    #[test]
    fn missing_network_names_both_flags() {
        let (mut args, _f) = paid_args_with_key(true);
        args.network = None;
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--network"), "got: {msg}");
        assert!(msg.contains("--payment-network"), "got: {msg}");
    }

    #[test]
    fn flag_key_file_beats_config_key_file() {
        let flag = key_file_with("0xfromflag\n");
        let cfg_f = key_file_with("0xfromconfig");
        let mut section = empty_section();
        section.key_file = Some(cfg_f.path().to_path_buf());
        let mut args = paid_args(true);
        args.payment_key_file = Some(flag.path().to_path_buf());
        let (cfg, _, _) = resolve_payment_config(&args, &section, None, None).unwrap();
        assert_eq!(cfg.key, "0xfromflag");
    }

    #[test]
    fn config_key_file_used_when_nothing_else() {
        let f = key_file_with("0xfromconfig");
        let mut section = empty_section();
        section.key_file = Some(f.path().to_path_buf());
        let args = paid_args(true);
        let (cfg, _, _) = resolve_payment_config(&args, &section, None, None).unwrap();
        assert_eq!(cfg.key, "0xfromconfig");
    }

    #[test]
    fn payment_wallet_flag_resolves_to_store_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("payer"), "0xfromwallet\n").unwrap();
        let mut args = paid_args(true);
        args.payment_wallet = Some("payer".to_string());
        let (cfg, _, _) =
            resolve_payment_config(&args, &empty_section(), Some(dir.path()), None).unwrap();
        assert_eq!(cfg.key, "0xfromwallet");
    }

    #[test]
    fn flag_key_file_beats_payment_wallet() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("payer"), "0xfromwallet").unwrap();
        let flag = key_file_with("0xfromflag");
        let mut args = paid_args(true);
        args.payment_key_file = Some(flag.path().to_path_buf());
        args.payment_wallet = Some("payer".to_string());
        let (cfg, _, _) =
            resolve_payment_config(&args, &empty_section(), Some(dir.path()), None).unwrap();
        assert_eq!(cfg.key, "0xfromflag");
    }

    #[test]
    fn config_wallet_used_as_last_resort() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("saved"), "0xfromcfgwallet").unwrap();
        let mut section = empty_section();
        section.wallet = Some("saved".to_string());
        let args = paid_args(true);
        let (cfg, _, _) = resolve_payment_config(&args, &section, Some(dir.path()), None).unwrap();
        assert_eq!(cfg.key, "0xfromcfgwallet");
    }

    #[test]
    fn unknown_payment_wallet_is_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = paid_args(true);
        args.payment_wallet = Some("ghost".to_string());
        let err =
            resolve_payment_config(&args, &empty_section(), Some(dir.path()), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wallet named 'ghost'"), "got: {msg}");
    }

    #[test]
    fn payment_wallet_rejects_unsafe_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = paid_args(true);
        args.payment_wallet = Some("../escape".to_string());
        let err =
            resolve_payment_config(&args, &empty_section(), Some(dir.path()), None).unwrap_err();
        assert!(
            err.to_string().contains("invalid wallet name"),
            "got: {err}"
        );
    }

    #[test]
    fn no_key_anywhere_is_actionable() {
        let args = paid_args(true);
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--payment-key-file"), "got: {msg}");
        assert!(msg.contains("--payment-wallet"), "got: {msg}");
        assert!(msg.contains("key_file"), "got: {msg}");
        assert!(!msg.contains("QN_PAYMENT_KEY"), "got: {msg}");
    }

    #[test]
    fn unreadable_key_file_names_the_path() {
        let mut args = paid_args(true);
        args.payment_key_file = Some("/does/not/exist.key".into());
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        assert!(err.to_string().contains("/does/not/exist.key"));
    }

    #[test]
    fn empty_key_file_is_rejected_without_leaking_contents() {
        let f = key_file_with("   \n");
        let mut args = paid_args(true);
        args.payment_key_file = Some(f.path().to_path_buf());
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        assert!(err.to_string().contains("empty"), "got: {err}");
    }

    #[test]
    fn inline_config_key_is_rejected_with_key_file_pointer() {
        let mut section = empty_section();
        section.key = Some(toml::Value::String("0xraw".to_string()));
        let (args, _f) = paid_args_with_key(true);
        let err = resolve_payment_config(&args, &section, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("key_file"), "got: {msg}");
        assert!(!msg.contains("0xraw"), "must not echo the key: {msg}");
    }

    #[test]
    fn missing_max_amount_is_actionable() {
        let (mut args, _f) = paid_args_with_key(true);
        args.max_amount = None;
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        assert!(err.to_string().contains("--max-amount"), "got: {err}");
    }

    #[test]
    fn non_integer_max_amount_is_rejected() {
        for bad in ["1.5", "abc", "-1", "1_000"] {
            let (mut args, _f) = paid_args_with_key(true);
            args.max_amount = Some(bad.to_string());
            let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
            assert!(err.to_string().contains("base units"), "for {bad}: {err}");
        }
    }

    #[test]
    fn flag_max_amount_beats_config() {
        let mut section = empty_section();
        section.max_amount = Some("999999".to_string());
        let (args, _f) = paid_args_with_key(true);
        let (cfg, _, _) = resolve_payment_config(&args, &section, None, None).unwrap();
        assert_eq!(cfg.max_amount, "10000");
    }

    #[test]
    fn config_fills_all_missing_params() {
        let f = key_file_with("0xk");
        let section = PaymentSection {
            key_file: Some(f.path().to_path_buf()),
            wallet: None,
            key: None,
            max_amount: Some("5000".to_string()),
            payment_network: Some("eip155:42431".to_string()),
            payment_asset: Some("0xdef".to_string()),
            svm_rpc_url: None,
        };
        let mut args = paid_args(false);
        args.max_amount = None;
        args.payment_network = None;
        args.payment_asset = None;
        let (cfg, _, _) = resolve_payment_config(&args, &section, None, None).unwrap();
        assert_eq!(cfg.scheme, "mpp");
        assert_eq!(cfg.max_amount, "5000");
        assert_eq!(cfg.pay_network, "eip155:42431");
        assert_eq!(cfg.asset, "0xdef");
    }

    #[test]
    fn base_url_override_is_threaded_through() {
        let (args, _f) = paid_args_with_key(true);
        let (cfg, _, _) = resolve_payment_config(
            &args,
            &empty_section(),
            None,
            Some("http://127.0.0.1:9999".to_string()),
        )
        .unwrap();
        assert_eq!(
            cfg.base_url_override.as_deref(),
            Some("http://127.0.0.1:9999")
        );
    }

    #[test]
    fn invalid_svm_rpc_url_is_rejected() {
        let (mut args, _f) = paid_args_with_key(true);
        args.svm_rpc_url = Some("ftp://nope".to_string());
        let err = resolve_payment_config(&args, &empty_section(), None, None).unwrap_err();
        assert!(err.to_string().contains("scheme"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_key_file_produces_warning() {
        use std::os::unix::fs::PermissionsExt;
        let f = key_file_with("0xk");
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let (_, warning) = read_key_file(f.path()).unwrap();
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert!(w.contains("chmod 600"), "got: {w}");
        assert!(!w.contains("0xk"), "must not leak contents: {w}");
    }

    #[cfg(unix)]
    #[test]
    fn private_key_file_produces_no_warning() {
        use std::os::unix::fs::PermissionsExt;
        let f = key_file_with("0xk");
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        let (_, warning) = read_key_file(f.path()).unwrap();
        assert!(warning.is_none(), "got: {warning:?}");
    }

    #[test]
    fn decode_error_maps_to_payment_maybe_charged() {
        let decode = SdkError::Decode {
            source: serde_json::from_str::<serde_json::Value>("x").unwrap_err(),
            body: "gateway 5xx html".to_string(),
        };
        let mapped = map_paid_error(decode);
        assert!(matches!(mapped, CliError::PaymentMaybeCharged(_)));
        assert_eq!(crate::errors::exit_code_for(&mapped), 3);
    }

    #[test]
    fn rejected_error_passes_through_to_exit_2() {
        let rejected = SdkError::PaymentRejected {
            status: 402,
            body: "bad sig".to_string(),
        };
        let mapped = map_paid_error(rejected);
        assert_eq!(crate::errors::exit_code_for(&mapped), 2);
    }

    #[test]
    fn rejected_5xx_maps_to_payment_maybe_charged() {
        for status in [500, 502, 503] {
            let rejected = SdkError::PaymentRejected {
                status,
                body: "settlement error".to_string(),
            };
            let mapped = map_paid_error(rejected);
            assert!(
                matches!(mapped, CliError::PaymentMaybeCharged(_)),
                "status {status} mapped to {mapped:?}"
            );
            assert_eq!(crate::errors::exit_code_for(&mapped), 3);
        }
    }
}
