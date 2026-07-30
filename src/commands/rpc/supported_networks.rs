//! `qn rpc {x402,mpp} supported-networks` and `supported-payments` — one
//! payment gateway's discovery lists, one per verb: the networks you can make
//! paid RPC calls **to**, and the payment options the gateway accepts (the
//! network, token, and contract address you pay **with**).
//!
//! Keyless and public: this reads the gateway's own discovery surfaces, not
//! the account API. Callable networks come from `{gateway}/networks`. Payment
//! options come from `x402.quicknode.com/supported` for x402; the MPP gateway
//! publishes them in the `WWW-Authenticate: Payment` challenge of a keyless
//! 402 response, so `supported-payments` probes one callable network without
//! paying. Each list is cached per scheme in `pay-networks.toml` next to the
//! config with a 24h TTL, mirroring the multichain URL cache. A callable
//! network is a valid `--network` for a paid call; a payment option's
//! network/address pair is a ready `--payment-network`/`--payment-asset`.

use std::collections::BTreeMap;
use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;

use crate::config::{self, PayAssetEntry};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, OutputCtx, Render};

const X402_BASE: &str = "https://x402.quicknode.com";
const MPP_BASE: &str = "https://mpp.quicknode.com";

/// Which payment gateway the command reads.
#[derive(Clone, Copy)]
pub(super) enum Scheme {
    X402,
    Mpp,
}

impl Scheme {
    fn as_str(self) -> &'static str {
        match self {
            Scheme::X402 => "x402",
            Scheme::Mpp => "mpp",
        }
    }

    fn gateway_base(self) -> &'static str {
        match self {
            Scheme::X402 => X402_BASE,
            Scheme::Mpp => MPP_BASE,
        }
    }
}

/// Cache/override plumbing shared by both verbs. A `--base-url` override
/// points the gateway fetches at one host (used by tests to serve the
/// discovery endpoints from a mock) and bypasses the cache, since a test
/// host's data isn't the real catalog.
struct Discovery {
    ctx: Ctx,
    scheme: Scheme,
    base: String,
    cache_path: Option<std::path::PathBuf>,
    use_cache: bool,
}

impl Discovery {
    fn new(scheme: Scheme, global: GlobalArgs) -> Result<Self, CliError> {
        let base_override = global.base_url.clone();
        let ctx = Ctx::from_global_keyless(global)?;
        let cache_path =
            config::pay_networks_cache_path(ctx.global.resolve_config_path().as_deref());
        let use_cache = base_override.is_none();
        let base = base_override.unwrap_or_else(|| scheme.gateway_base().to_string());
        Ok(Self {
            ctx,
            scheme,
            base,
            cache_path,
            use_cache,
        })
    }

    fn cache_path(&self) -> Option<&Path> {
        self.use_cache
            .then_some(self.cache_path.as_deref())
            .flatten()
    }

    /// The callable-networks list: fresh cache hit, or fetch + cache.
    async fn ensure_networks(&self, client: &reqwest::Client) -> Result<Vec<String>, CliError> {
        if let Some(cached) = self
            .cache_path()
            .and_then(|p| config::load_pay_networks(p, self.scheme.as_str(), now_unix()))
        {
            return Ok(cached);
        }
        let networks = fetch_networks(client, &self.base).await?;
        if let Some(p) = self.cache_path() {
            let _ = config::save_pay_networks(p, self.scheme.as_str(), now_unix(), &networks);
        }
        Ok(networks)
    }

    /// The payment-options list: fresh cache hit, or fetch + cache. The MPP
    /// probe needs a callable network, which reuses the networks cache.
    async fn ensure_payments(
        &self,
        client: &reqwest::Client,
    ) -> Result<Vec<PayAssetEntry>, CliError> {
        if let Some(cached) = self
            .cache_path()
            .and_then(|p| config::load_pay_payments(p, self.scheme.as_str(), now_unix()))
        {
            return Ok(cached);
        }
        let payments = match self.scheme {
            Scheme::X402 => fetch_x402_payments(client, &self.base).await?,
            Scheme::Mpp => {
                let networks = self.ensure_networks(client).await?;
                fetch_mpp_payments(client, &self.base, networks.first()).await?
            }
        };
        if let Some(p) = self.cache_path() {
            let _ = config::save_pay_payments(p, self.scheme.as_str(), now_unix(), &payments);
        }
        Ok(payments)
    }
}

/// `qn rpc {x402,mpp} supported-networks`.
pub(super) async fn run_networks(scheme: Scheme, global: GlobalArgs) -> Result<(), CliError> {
    let d = Discovery::new(scheme, global)?;
    let networks = d.ensure_networks(&reqwest::Client::new()).await?;
    crate::output::emit(&d.ctx.out, &NetworksView(networks))
}

/// `qn rpc {x402,mpp} supported-payments`.
pub(super) async fn run_payments(scheme: Scheme, global: GlobalArgs) -> Result<(), CliError> {
    let d = Discovery::new(scheme, global)?;
    let payments = d.ensure_payments(&reqwest::Client::new()).await?;
    crate::output::emit(&d.ctx.out, &PaymentsView(payments))
}

/// GET `{base}/networks` → the `networks` slug array.
async fn fetch_networks(client: &reqwest::Client, base: &str) -> Result<Vec<String>, CliError> {
    #[derive(Deserialize)]
    struct NetworksResp {
        networks: Vec<String>,
    }
    let url = format!("{base}/networks");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| fetch_err(&url, e))?;
    if !resp.status().is_success() {
        return Err(CliError::Arg(format!(
            "discovery request to {url} failed with HTTP {}",
            resp.status().as_u16()
        )));
    }
    let body: NetworksResp = resp.json().await.map_err(|e| fetch_err(&url, e))?;
    Ok(body.networks)
}

/// GET x402 `/supported` → the accepted payment options, deduplicated by
/// (network, token address). The display name prefers our known symbol table;
/// otherwise the offer's `extra.name`, but only from offers without a
/// `verifyingContract` (those are Circle Gateway variants whose `name` is an
/// EIP-712 domain, not a token).
async fn fetch_x402_payments(
    client: &reqwest::Client,
    base: &str,
) -> Result<Vec<PayAssetEntry>, CliError> {
    #[derive(Deserialize)]
    struct Supported {
        #[serde(default)]
        accepts: Vec<Accept>,
    }
    #[derive(Deserialize)]
    struct Accept {
        network: String,
        #[serde(default)]
        asset: Option<String>,
        #[serde(default)]
        extra: Option<Extra>,
    }
    #[derive(Deserialize)]
    struct Extra {
        #[serde(default)]
        name: Option<String>,
        #[serde(default, rename = "verifyingContract")]
        verifying_contract: Option<String>,
    }

    let url = format!("{base}/supported");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| fetch_err(&url, e))?;
    // The gateway serves this catalog x402-style: the payment-requirements
    // JSON arrives with HTTP 402, not 200. Both carry the same shape.
    let status = resp.status();
    if !status.is_success() && status.as_u16() != 402 {
        return Err(CliError::Arg(format!(
            "discovery request to {url} failed with HTTP {}",
            status.as_u16()
        )));
    }
    let body: Supported = resp.json().await.map_err(|e| fetch_err(&url, e))?;

    // (CAIP-2 network, address) → best-known display name.
    let mut merged: BTreeMap<(String, String), Option<String>> = BTreeMap::new();
    for accept in body.accepts {
        let Some(address) = accept.asset else {
            continue;
        };
        let offer_name = match accept.extra {
            Some(Extra {
                verifying_contract: None,
                name,
            }) => name,
            _ => None,
        };
        let name = super::pay_asset::symbol_for(&accept.network, &address).or(offer_name);
        let slot = merged.entry((accept.network.clone(), address)).or_default();
        if slot.is_none() {
            *slot = name;
        }
    }

    let mut out: Vec<PayAssetEntry> = merged
        .into_iter()
        .map(|((caip2, address), asset)| PayAssetEntry {
            network: super::pay_network::slug_for_caip2(&caip2).unwrap_or(caip2),
            asset,
            address,
        })
        .collect();
    out.sort_by(|a, b| (&a.network, &a.address).cmp(&(&b.network, &b.address)));
    Ok(out)
}

/// The MPP gateway lists its accepted payment options only in the 402
/// challenge, so probe one callable network with a keyless request (no payment
/// is taken) and parse the `WWW-Authenticate: Payment` header.
async fn fetch_mpp_payments(
    client: &reqwest::Client,
    base: &str,
    probe_slug: Option<&String>,
) -> Result<Vec<PayAssetEntry>, CliError> {
    let Some(slug) = probe_slug else {
        return Err(CliError::Arg(
            "the gateway listed no callable networks to probe".to_string(),
        ));
    };
    let url = format!("{base}/{slug}");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "eth_blockNumber", "params": []
        }))
        .send()
        .await
        .map_err(|e| fetch_err(&url, e))?;
    let status = resp.status().as_u16();
    let header = resp
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            CliError::Arg(format!(
                "the probe of {url} returned HTTP {status} without a payment challenge"
            ))
        })?;
    let challenges = parse_payment_challenges(header);
    if challenges.is_empty() {
        return Err(CliError::Arg(format!(
            "could not parse the payment challenge from {url}"
        )));
    }

    // (network slug, address) → best-known display name.
    let mut merged: BTreeMap<(String, String), Option<String>> = BTreeMap::new();
    for ch in challenges {
        let Some(address) = ch.request.get("currency").and_then(|v| v.as_str()) else {
            continue;
        };
        let details = ch.request.get("methodDetails");
        let slug = match ch.method.as_str() {
            "tempo" => {
                let Some(id) = details
                    .and_then(|d| d.get("chainId"))
                    .and_then(|v| v.as_i64())
                else {
                    continue;
                };
                let caip2 = format!("eip155:{id}");
                super::pay_network::slug_for_caip2(&caip2).unwrap_or(caip2)
            }
            "solana" => {
                match details
                    .and_then(|d| d.get("network"))
                    .and_then(|v| v.as_str())
                {
                    Some("mainnet-beta") => "solana-mainnet".to_string(),
                    Some("devnet") => "solana-devnet".to_string(),
                    Some("testnet") => "solana-testnet".to_string(),
                    Some(other) => other.to_string(),
                    None => continue,
                }
            }
            _ => continue,
        };
        let caip2 = super::pay_network::resolve(&slug).unwrap_or_else(|_| slug.clone());
        let name = super::pay_asset::symbol_for(&caip2, address);
        let slot = merged.entry((slug, address.to_string())).or_default();
        if slot.is_none() {
            *slot = name;
        }
    }

    Ok(merged
        .into_iter()
        .map(|((network, address), asset)| PayAssetEntry {
            network,
            asset,
            address,
        })
        .collect())
}

/// One parsed `Payment` challenge: its `method` and decoded `request` JSON.
struct Challenge {
    method: String,
    request: serde_json::Value,
}

/// Parses a `WWW-Authenticate` header carrying one or more `Payment`
/// challenges (`Payment k="v", k="v", ..., Payment k="v", ...`). Pragmatic,
/// not a full RFC 7235 grammar: a `Payment ` prefix on a comma-separated part
/// starts a new challenge, and only `method` and `request` are read. Malformed
/// entries are skipped.
fn parse_payment_challenges(header: &str) -> Vec<Challenge> {
    let mut out = Vec::new();
    let mut method: Option<String> = None;
    let mut request: Option<String> = None;
    for raw in split_outside_quotes(header, ',') {
        let mut part = raw.trim();
        if let Some(rest) = part.strip_prefix("Payment ") {
            if let Some(c) = decode_challenge(method.take(), request.take()) {
                out.push(c);
            }
            part = rest.trim_start();
        }
        if let Some(v) = part.strip_prefix("method=") {
            method = unquote(v);
        } else if let Some(v) = part.strip_prefix("request=") {
            request = unquote(v);
        }
    }
    if let Some(c) = decode_challenge(method, request) {
        out.push(c);
    }
    out
}

/// Splits on `delim` occurrences outside double-quoted spans — quoted header
/// values may themselves contain commas.
fn split_outside_quotes(s: &str, delim: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c == delim && !in_quotes => {
                parts.push(&s[start..i]);
                start = i + delim.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn unquote(s: &str) -> Option<String> {
    s.strip_prefix('"')?.strip_suffix('"').map(str::to_string)
}

/// Base64url-decodes a challenge's `request` value into JSON. `None` on any
/// decode failure, so one malformed challenge never sinks the others.
fn decode_challenge(method: Option<String>, request: Option<String>) -> Option<Challenge> {
    let method = method?;
    let request = request?;
    let bytes = URL_SAFE_NO_PAD.decode(request.trim_end_matches('=')).ok()?;
    let json = serde_json::from_slice(&bytes).ok()?;
    Some(Challenge {
        method,
        request: json,
    })
}

/// `supported-networks` output: a bare array in JSON, one NETWORK column as a
/// table.
#[derive(serde::Serialize)]
#[serde(transparent)]
struct NetworksView(Vec<String>);

impl Render for NetworksView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        if self.0.is_empty() {
            return writeln!(w, "(none listed)");
        }
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["NETWORK"]);
        for n in &self.0 {
            t.add_row(vec![comfy_table::Cell::new(n)]);
        }
        write_table(w, &t)
    }
}

/// `supported-payments` output: a bare array in JSON, NETWORK/ASSET/ADDRESS
/// columns as a table.
#[derive(serde::Serialize)]
#[serde(transparent)]
struct PaymentsView(Vec<PayAssetEntry>);

impl Render for PaymentsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        if self.0.is_empty() {
            return writeln!(w, "(none listed)");
        }
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["NETWORK", "ASSET", "ADDRESS"]);
        for e in &self.0 {
            t.add_row(vec![
                comfy_table::Cell::new(&e.network),
                crate::output::opt_cell(&e.asset),
                comfy_table::Cell::new(&e.address),
            ]);
        }
        write_table(w, &t)
    }
}

fn fetch_err(url: &str, e: reqwest::Error) -> CliError {
    CliError::Arg(format!(
        "could not fetch the gateway catalog from {url}: {e}"
    ))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(json: &str) -> String {
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    #[test]
    fn parses_multi_challenge_header() {
        let tempo =
            b64(r#"{"amount":"1000","currency":"0xabc","methodDetails":{"chainId":42431}}"#);
        let solana = b64(
            r#"{"amount":"0.001","currency":"Mint111","methodDetails":{"network":"mainnet-beta"}}"#,
        );
        // The description stresses the splitter: a quoted comma and the word
        // "Payment" inside a value must not start a new challenge.
        let header = format!(
            "Payment id=\"a\", realm=\"r\", method=\"tempo\", intent=\"charge\", \
             request=\"{tempo}\", description=\"Payment, per request\", \
             Payment id=\"b\", method=\"solana\", request=\"{solana}\""
        );
        let out = parse_payment_challenges(&header);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].method, "tempo");
        assert_eq!(out[0].request["methodDetails"]["chainId"], 42431);
        assert_eq!(out[1].method, "solana");
        assert_eq!(out[1].request["currency"], "Mint111");
    }

    #[test]
    fn skips_malformed_entries() {
        let good = b64(r#"{"currency":"0xabc","methodDetails":{"chainId":4217}}"#);
        // First challenge has an undecodable request; second is fine.
        let header = format!(
            "Payment method=\"tempo\", request=\"!!!not-base64!!!\", \
             Payment method=\"tempo\", request=\"{good}\""
        );
        let out = parse_payment_challenges(&header);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].request["methodDetails"]["chainId"], 4217);
    }

    #[test]
    fn header_without_challenges_parses_empty() {
        assert!(parse_payment_challenges("Bearer realm=\"api\"").is_empty());
    }
}
