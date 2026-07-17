//! The crypto-micropayment lane of `qn rpc call` (`--x402`/`--mpp`): pay per
//! RPC request with a stablecoin instead of an account API key, via the SDK's
//! 402 → sign → resend handshake against Quicknode's payment gateways.
//!
//! This lane is deliberately structural, not conditional: `run_call` branches
//! here before any of the default lane's machinery, so the token cache, the
//! Tooling Access enable/probe recovery, the networks map, and `retrying()`
//! are unreachable. Paid calls are NEVER auto-retried — a lost response after
//! the payment was submitted (`PaymentIndeterminate`, or an uninterpretable
//! post-payment body) means the caller may already have been charged.
//!
//! Everything the lane needs is resolved before any network I/O: the private
//! key (flag file/stdin > `--payment-wallet` > `key_file` > `wallet` in config
//! — always from a file, never an env var or a raw key on the command line or
//! inline in config), the spend ceiling, the pay network, and the asset. The
//! key lives only inside the SDK's `PaymentConfig` (which redacts it in Debug)
//! and is never logged or echoed.

use std::path::Path;

use quicknode_sdk::errors::SdkError;
use quicknode_sdk::PaymentConfig;

use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;

use super::CallArgs;

/// Entry point from `run_call` once `--x402`/`--mpp` is present.
pub(super) async fn run_paid_call(args: CallArgs, global: GlobalArgs) -> Result<(), CliError> {
    // Both the key and the params may come from stdin; there is only one stdin.
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

    // Config parameter defaults. Unlike the default lane's endpoint_url load,
    // a broken config file is a hard error here (exit 4): the user probably
    // relies on [rpc.payment] values we could not read.
    let section = load_payment_section(&global)?;

    // The wallet store directory backs `--payment-wallet` / config `wallet`.
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

    // ONE attempt, no retrying(): a retried paid call risks a double charge.
    // call_with_receipt is the single code path; the receipt is dropped unless
    // --receipt asked for it.
    let resp = ctx
        .sdk
        .rpc
        .call_with_receipt(&args.method, params, Some(network), None)
        .await
        .map_err(map_paid_error)?;

    if args.receipt {
        // The receipt is data: it goes to stdout, opted into explicitly since
        // it changes the output shape. `null` on x402 (no settlement
        // reference exists in that protocol).
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
        // Identical output shape to an unpaid call.
        super::emit_result(&ctx, &resp.result)
    }
}

/// Loads `[rpc.payment]` from the resolved config file. A missing file is an
/// empty section; an unreadable/invalid file is a hard error, since payment
/// parameters the user set there would otherwise be silently ignored.
fn load_payment_section(global: &GlobalArgs) -> Result<PaymentSection, CliError> {
    let Some(path) = global.resolve_config_path() else {
        return Ok(PaymentSection::default());
    };
    Ok(config::load_from(&path)?
        .map(|cfg| cfg.rpc.payment)
        .unwrap_or_default())
}

/// Resolves the full payment configuration from flags, the injected env value,
/// and the config section — entirely before any network I/O, so every missing
/// or malformed input fails fast with an actionable message and zero requests
/// sent. Returns the SDK config, the query network, and an optional key-file
/// permissions warning for the caller to print once output exists.
/// The payment parameter stack shared by `qn rpc call --x402/--mpp` and the
/// gateway lifecycle verbs (`qn rpc x402 …`, `qn rpc mpp …`). Plain data,
/// resolved from flags (or the verb's own args) with `[rpc.payment]` fallback
/// by [`resolve_payment_params`].
pub(super) struct PaymentParams<'a> {
    pub key_file: Option<&'a Path>,
    pub wallet: Option<&'a str>,
    pub max_amount: Option<&'a str>,
    pub payment_network: Option<&'a str>,
    pub payment_asset: Option<&'a str>,
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
    // Scheme comes from which flag is set; clap's ArgGroup guarantees at most
    // one, and run_paid_call only runs when one is present.
    let scheme = if args.x402 { "x402" } else { "mpp" };

    // The query chain. Required, but enforced here rather than via clap so the
    // error can explain the query-chain vs pay-chain distinction.
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

/// Resolves the shared payment parameter stack (key, spend ceiling, pay
/// network, asset, SVM RPC URL) into an SDK [`PaymentConfig`] for `scheme`,
/// applying the flags-then-`[rpc.payment]` precedence. Returns the config plus
/// an optional key-file permissions warning. Does not touch the query network
/// (that is call-specific).
pub(super) fn resolve_payment_params(
    scheme: &str,
    params: &PaymentParams<'_>,
    section: &PaymentSection,
    wallets_dir: Option<&Path>,
    base_url_override: Option<String>,
) -> Result<(PaymentConfig, Option<String>), CliError> {
    // An inline raw key in config is never accepted — it belongs in a file.
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

/// Resolves the raw private key from a file only — never an env var and never
/// a raw key on the command line. Precedence: `--payment-key-file` (a path, or
/// `-` for stdin) > `--payment-wallet` (a stored wallet name) > config
/// `key_file` > config `wallet`. Returns the key and an optional permissions
/// warning (group/world-readable key file). Error messages name the path or
/// wallet, never the file contents.
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
        return read_key_file(&wallet_key_path(name, wallets_dir)?);
    }
    if let Some(path) = config_file {
        return read_key_file(path);
    }
    if let Some(name) = config_wallet {
        return read_key_file(&wallet_key_path(name, wallets_dir)?);
    }
    Err(CliError::Arg(
        "no payment key found. Pass --payment-key-file <PATH> (or '-' for \
         stdin), --payment-wallet <NAME> (from 'qn rpc wallet generate'), or \
         set `key_file`/`wallet` under [rpc.payment]"
            .to_string(),
    ))
}

/// Resolves a stored wallet name to its key file path, validating the name and
/// checking the file exists. Mirrors the `wallet` module's name rules so the
/// name can never escape the store directory.
fn wallet_key_path(name: &str, wallets_dir: Option<&Path>) -> Result<std::path::PathBuf, CliError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(CliError::Arg(format!(
            "invalid wallet name '{name}'. Use lowercase letters, digits, '-' and '_' only"
        )));
    }
    let dir = wallets_dir
        .ok_or_else(|| CliError::Arg("could not resolve the wallet store directory".to_string()))?;
    let path = dir.join(name);
    if !path.exists() {
        return Err(CliError::Arg(format!(
            "no wallet named '{name}'. Run 'qn rpc wallet list' to see stored wallets, \
             or create one with 'qn rpc wallet generate'"
        )));
    }
    Ok(path)
}

/// Reads and validates a key file, plus an ssh-style permissions warning when
/// the file is group- or world-readable.
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

/// Trims and rejects an empty key. `source` names where the key came from
/// (a path, `stdin`, or the env var) — never its contents.
fn checked_key(raw: String, source: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::Arg(format!(
            "the payment key from {source} is empty"
        )));
    }
    Ok(trimmed.to_string())
}

/// Maps a paid-call failure onto the paid exit-code contract: 2 = the gateway
/// refused and nothing settled, 3 = outcome unknown, check the wallet.
///
/// - `Decode` on the paid lane always means the gateway's post-payment 2xx
///   response could not be interpreted (the SDK classifies pre-payment parse
///   failures as `PaymentUnsupported`/`Config`) — the payment may already
///   have settled, so it gets the same never-blindly-retry treatment as
///   `PaymentIndeterminate` (exit 3).
/// - `PaymentRejected` with a 5xx status is a gateway/settlement failure
///   after the signed payment was submitted — also unknown, exit 3. A 4xx
///   rejection means the gateway refused the credential without settling it
///   and passes through to exit 2.
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

    /// `paid_args` with a valid key file attached, so a test can reach the
    /// parameter-validation assertions without caring about the key source.
    /// Returns the (args, key-file guard) pair — keep the guard alive.
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
        assert_eq!(cfg.key, "0xkey"); // trimmed
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
        assert_eq!(cfg.key, "0xfromflag"); // and trimmed
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
        // The env var is gone; the message must not resurrect it.
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
        let (args, _f) = paid_args_with_key(true); // flag says 10000
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
        // A settlement failure after the signed payment was submitted: the
        // outcome is unknown, so it must NOT land in the "refused" bucket.
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
