//! `qn rpc call <method> [params]` — make a JSON-RPC call against the account's
//! Tooling Access endpoint (or a custom URL), and `qn rpc list-networks` — list
//! the multichain endpoint's available network keys.
//!
//! By default there's no endpoint URL or token to manage: the SDK mints and
//! refreshes a short-lived session JWT automatically. Because each CLI
//! invocation is a fresh process, we persist that JWT to `tokens.toml` and
//! re-seed it next time (see `crate::config` token cache), so a valid token
//! means no control-plane round trip.
//!
//! On a never-provisioned account the first call fails with "not enabled"; we
//! offer to enable (prompt on a TTY, `--yes` for scripts/agents, otherwise an
//! actionable error), then retry.
//!
//! A custom endpoint URL (`--endpoint-url` per call, or `[rpc] endpoint_url` in
//! config) is a separate lane: the SDK sends the call straight to that URL with
//! no JWT minted or attached. That lane never touches the token cache or the
//! Tooling Access enable/probe recovery.

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
        qn rpc call eth_getBalance '[\"0xabc...\", \"latest\"]'\n  \
        qn rpc call getSlot --network solana-mainnet\n  \
        qn rpc call eth_blockNumber --endpoint-url https://my-endpoint.example/rpc\n  \
        qn rpc call eth_call --params-file params.json\n  \
        echo '[...]' | qn rpc call eth_call -\n  \
        cat params.json | qn rpc call eth_call -f -")]
    Call(CallArgs),

    /// List the endpoint's available network keys (no RPC call).
    #[command(visible_alias = "ls")]
    ListNetworks,
}

#[derive(Debug, ClapArgs)]
#[command(group(ArgGroup::new("params_source").args(["params", "params_file"])))]
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
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        RpcCmd::Call(call) => run_call(call, global).await,
        RpcCmd::ListNetworks => run_list_networks(global).await,
    }
}

/// `qn rpc list-networks` — always targets the account's Tooling Access
/// multichain endpoint (a custom `endpoint_url` has no network map), so it
/// ignores any configured or flagged custom URL.
async fn run_list_networks(global: GlobalArgs) -> Result<(), CliError> {
    let config_path = global.resolve_config_path();
    let networks_path = config::networks_cache_path(config_path.as_deref());
    let token_path = config::token_cache_path(config_path.as_deref());
    let seed = match (&token_path, resolve_key_quietly(&global)) {
        (Some(p), Some(key)) => config::load_token(p, &key),
        _ => None,
    };
    let (ctx, _api_key) = Ctx::from_global_with_rpc_seed(global, seed, None)?;
    let map = ensure_networks(&ctx, networks_path.as_deref()).await?;
    emit_networks(&ctx, &map)
}

async fn run_call(args: CallArgs, global: GlobalArgs) -> Result<(), CliError> {
    let params = parse_params(args.params.as_deref(), args.params_file.as_deref())?;

    // A custom URL (per-call flag, else the `[rpc] endpoint_url` config default)
    // is a separate lane: no JWT, no token cache, no Tooling Access recovery.
    // Validate the flag here; the config value is validated when `Ctx` builds it.
    let flag_endpoint_url = match args.endpoint_url.as_deref() {
        Some(u) => Some(crate::context::validate_endpoint_url(u)?),
        None => None,
    };
    let config_endpoint_url = load_config_endpoint_url(&global);
    let custom_url = flag_endpoint_url
        .clone()
        .or_else(|| config_endpoint_url.clone());

    // Load any cached token to seed the SDK and avoid a mint round trip. We need
    // the resolved API key to scope the cache, so resolve the config path first.
    let config_path = global.resolve_config_path();
    let token_path = config::token_cache_path(config_path.as_deref());
    let networks_path = config::networks_cache_path(config_path.as_deref());

    // We don't know the API key until Ctx builds it, but the cache is keyed by
    // the key's fingerprint. Resolve it once here for the load; Ctx re-resolves
    // and returns it for the write-back (cheap, and keeps Ctx the source of truth).
    let seed = match (&token_path, resolve_key_quietly(&global)) {
        (Some(p), Some(key)) => config::load_token(p, &key),
        _ => None,
    };

    let (ctx, api_key) = Ctx::from_global_with_rpc_seed(global, seed, config_endpoint_url)?;

    // Multichain selection (--network) needs the per-network URL map. Resolve it
    // lazily — only when a network is involved — so the common default-network
    // call path stays a single round trip. (A custom URL conflicts with
    // --network at the clap layer, so this only runs on the Tooling Access lane.)
    if args.network.is_some() {
        let map = ensure_networks(&ctx, networks_path.as_deref()).await?;
        ctx.sdk.rpc.set_networks(map);
    }

    let method = args.method.as_str();

    // First attempt. When a custom URL is active, the call goes straight there
    // (self-authenticating) and any error surfaces directly — the Tooling Access
    // enable/probe recovery below only applies to the token-minting lane.
    //
    // On the Tooling Access lane, both ways of discovering "disabled" converge on
    // the same flow: offer to enable (y/N on a TTY, --yes to auto-enable,
    // actionable error otherwise), then retry.
    //   - mint returns the "not enabled" 400 (no usable token), or
    //   - the call connect/timeouts against a stale-but-unexpired cached token
    //     and a status probe confirms the endpoint is disabled (possibly
    //     out-of-band). That path also clears the stale token first.
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
                // The stale token points at the disabled endpoint. Drop it both
                // in memory (so this retry mints fresh) and on disk (so the next
                // process does too) before enabling and retrying.
                ctx.sdk.rpc.clear_cached_token();
                if let Some(p) = &token_path {
                    let _ = config::delete_config(p);
                }
                maybe_enable(&ctx).await?;
                call_after_enable(&ctx, method, &params, args.network.clone()).await?
            } else {
                // Status enabled (real endpoint blip) or the probe itself failed
                // (genuine network issue): the honest transport error.
                return Err(e);
            }
        }
        Err(e) => return Err(map_unknown_network(e)),
    };

    // Snapshot the (possibly refreshed) token and write it back. On the custom-URL
    // lane no token is minted, so `current_token()` is `None` and nothing is
    // written — the existing cache is left untouched. Otherwise we always persist
    // when a token is present: the write is an idempotent atomic replace, the
    // token is short-lived, and re-writing an unchanged token is harmless.
    // Best-effort — a cache write failure must not fail the call, which already
    // succeeded; the next run simply re-mints.
    if let (Some(p), Some(current)) = (&token_path, ctx.sdk.rpc.current_token()) {
        let _ = config::save_token(p, &api_key, &current);
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

/// Returns the endpoint's per-network URL map (key -> http_url), using the
/// `networks.toml` cache when fresh (24h TTL) and otherwise fetching it via
/// `get_endpoint_urls` and rewriting the cache. Requires Tooling Access to be
/// enabled (the endpoint id comes from status).
async fn ensure_networks(
    ctx: &Ctx,
    networks_path: Option<&std::path::Path>,
) -> Result<std::collections::HashMap<String, String>, CliError> {
    // Need the endpoint id to scope the cache and fetch URLs.
    let status = retrying(ctx.global.retries, || ctx.sdk.admin.tooling_access_status()).await?;
    let Some(endpoint_id) = status.endpoint_id else {
        return Err(CliError::Arg(
            "this account's Tooling Access endpoint did not report an id, so per-network \
             routing is unavailable. Omit --network to use the default network."
                .to_string(),
        ));
    };

    // Fresh cache hit?
    if let Some(p) = networks_path {
        if let Some(map) = config::load_networks(p, &endpoint_id, now_unix()) {
            return Ok(map);
        }
    }

    // Miss/stale: fetch and rewrite.
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
    // Default to a plain one-key-per-line list (friendly for discovery, TTY or
    // piped). Only an explicit structured format produces the JSON envelope.
    if matches!(ctx.global.format, Some(f) if f.is_structured()) {
        let v = serde_json::json!({ "networks": keys });
        return emit_result(ctx, &v);
    }
    for k in keys {
        println!("{k}");
    }
    Ok(())
}

/// Resolve the API key without prompting, swallowing errors (the real
/// resolution + error happens in `Ctx::build`). Used only to scope the cache
/// load before `Ctx` is constructed.
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
    // RPC reads are safe to retry on transient transport failures, same as
    // other read-only commands, on both the Tooling Access and custom-URL lanes.
    // The SDK handles its own one-shot 401 refresh (Tooling Access lane only).
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

// Total wall-clock budget for retrying a call right after enabling Tooling
// Access, while the freshly-provisioned endpoint host becomes routable.
const POST_ENABLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
// A just-enabled endpoint is essentially never reachable on the first instant;
// a short initial wait absorbs the common case before we even try.
const POST_ENABLE_INITIAL_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Retry the call right after enabling Tooling Access, tolerating the
/// provisioning lag where the new endpoint host isn't routable yet. Waits 1s
/// first, then retries transient (connect/timeout/5xx/429) failures with
/// exponential backoff until `POST_ENABLE_BUDGET` (~10s) is exhausted. A
/// non-transient error (e.g. a JSON-RPC or 4xx) returns immediately.
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
        // Post-enable retry only runs on the Tooling Access lane (a custom URL
        // bypasses the enable flow entirely), so no custom endpoint_url here.
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
                // Don't sleep past the deadline.
                let remaining = deadline - now;
                tokio::time::sleep(backoff.min(remaining)).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(4));
            }
        }
    }
}

/// True when the error is the control plane's "Tooling Access not enabled" 400.
fn is_not_enabled(err: &CliError) -> bool {
    matches!(
        err,
        CliError::Sdk(quicknode_sdk::errors::SdkError::Api { status, body })
            if status.as_u16() == 400 && body.to_lowercase().contains("not enabled")
    )
}

/// True for a connect/timeout transport failure (as opposed to an HTTP status
/// or JSON-RPC error). These are the ambiguous failures worth probing status for.
fn is_transport_failure(err: &CliError) -> bool {
    use quicknode_sdk::errors::HttpKind;
    matches!(
        err,
        CliError::Sdk(sdk @ quicknode_sdk::errors::SdkError::Http(_))
            if matches!(sdk.http_kind(), Some(HttpKind::Connect | HttpKind::Timeout))
    )
}

/// Probe `tooling_access_status` to disambiguate an RPC connect/timeout failure.
/// Returns true only if the probe succeeds and reports the endpoint disabled
/// (the case we can act on). A probe that fails (genuine network issue) or
/// reports enabled (real endpoint blip) returns false, so the caller surfaces
/// the original transport error. No retries: best-effort diagnosis on an
/// already-failed call.
async fn disabled_per_status(ctx: &Ctx) -> bool {
    matches!(ctx.sdk.admin.tooling_access_status().await, Ok(s) if !s.enabled)
}

/// Offer to enable Tooling Access: auto on `--yes`, prompt on a TTY, and an
/// actionable error in non-interactive contexts. Mirrors the confirmation
/// gating used for destructive ops (`crate::confirm`).
async fn maybe_enable(ctx: &Ctx) -> Result<(), CliError> {
    // The global -y/--yes (count) is consent to provision without prompting.
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );

    let proceed = match decide_without_prompt(Severity::Mild, cfg) {
        Ok(p) => p,
        // Non-interactive without consent: point the user at the explicit command.
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

    // enable mutates account state — never retry. enable() surfaces the typed
    // reason (e.g. admin-role / eligibility 4xx) which renders actionably.
    ctx.sdk.admin.enable_tooling_access().await?;
    ctx.out.note("✓ Enabled Tooling Access");
    Ok(())
}

/// Resolve params from the inline positional arg or `--params-file`, then parse
/// as JSON. `None`/`None` → no params (sends `[]`). Either source accepts `-` to
/// read from stdin. The clap `ArgGroup` guarantees at most one is set. An empty
/// value (after trimming) is treated as no params.
fn parse_params(arg: Option<&str>, file: Option<&Path>) -> Result<Option<Value>, CliError> {
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
fn read_stdin(what: &str) -> Result<String, CliError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| CliError::Arg(format!("could not read {what} from stdin: {e}")))?;
    Ok(buf)
}

/// Emit the RPC result. RPC results are schemaless JSON, so JSON is the real
/// default: a bare `qn rpc` prints JSON whether on a TTY or piped (we read the
/// raw `--format` flag, not the TTY-aware resolved default). `json`/`yaml`/`toon`
/// render as requested. `table`/`md` have no columns here, so they fall back to
/// JSON — and only that explicit case prints a one-line note on stderr.
fn emit_result(ctx: &Ctx, result: &Value) -> Result<(), CliError> {
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
            // The user explicitly asked for a tabular format, which has no
            // columns for schemaless RPC output; say so once, then print JSON.
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
