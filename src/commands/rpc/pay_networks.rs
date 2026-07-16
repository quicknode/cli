//! `qn rpc pay-networks` — list the networks payable via the crypto
//! micropayment lane (`--x402`/`--mpp`).
//!
//! Keyless and public: this fetches the gateways' own discovery endpoints
//! (`x402.quicknode.com/networks` + `/discovery/resources`, and
//! `mpp.quicknode.com/networks`) directly, not through the SDK — those hosts
//! are the payment gateways, not the account API. Results are cached in
//! `pay-networks.toml` next to the config with a 24h TTL, mirroring the
//! multichain URL cache. A `--network` value from this list is a valid
//! `--network` slug for a paid call; the x402 `asset` column (when shown) is a
//! ready value for `--asset`.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::{self, PayNetworkEntry};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, OutputCtx, Render};

const X402_BASE: &str = "https://x402.quicknode.com";
const MPP_BASE: &str = "https://mpp.quicknode.com";

pub(super) async fn run(global: GlobalArgs) -> Result<(), CliError> {
    // A `--base-url` override points both gateway fetches at one host (used by
    // tests to serve the discovery endpoints from a mock). It also bypasses the
    // cache, since a test host's data isn't the real catalog.
    let base_override = global.base_url.clone();
    let ctx = Ctx::from_global_keyless(global)?;
    let cache_path = config::pay_networks_cache_path(ctx.global.resolve_config_path().as_deref());

    // Fresh cache hit? (Skipped when a --base-url override is active.)
    let cached = if base_override.is_none() {
        cache_path
            .as_deref()
            .and_then(|p| config::load_pay_networks(p, now_unix()))
    } else {
        None
    };

    let networks = match cached {
        Some(n) => n,
        None => {
            let (x402_base, mpp_base) = match &base_override {
                Some(b) => (b.as_str(), b.as_str()),
                None => (X402_BASE, MPP_BASE),
            };
            let fetched = fetch_pay_networks(x402_base, mpp_base).await?;
            if base_override.is_none() {
                if let Some(p) = cache_path.as_deref() {
                    let _ = config::save_pay_networks(p, now_unix(), &fetched);
                }
            }
            fetched
        }
    };

    crate::output::emit(&ctx.out, &PayNetworksView(networks))
}

/// Fetches and merges the payable networks from both gateways. x402 networks
/// are enriched with the asset from the Bazaar discovery catalog when present.
async fn fetch_pay_networks(
    x402_base: &str,
    mpp_base: &str,
) -> Result<Vec<PayNetworkEntry>, CliError> {
    let client = reqwest::Client::new();

    // BTreeMap keeps the merged list sorted by slug for a stable render.
    let mut merged: BTreeMap<String, PayNetworkEntry> = BTreeMap::new();

    let x402 = fetch_networks(&client, x402_base).await?;
    for net in x402 {
        merged.insert(
            net.clone(),
            PayNetworkEntry {
                network: net,
                schemes: vec!["x402".to_string()],
                asset: None,
            },
        );
    }

    let mpp = fetch_networks(&client, mpp_base).await?;
    for net in mpp {
        merged
            .entry(net.clone())
            .and_modify(|e| e.schemes.push("mpp".to_string()))
            .or_insert(PayNetworkEntry {
                network: net,
                schemes: vec!["mpp".to_string()],
                asset: None,
            });
    }

    // Enrich x402 networks with the asset from the discovery catalog. This is
    // best-effort: a fetch/parse failure leaves the asset column blank rather
    // than failing the whole command.
    if let Ok(assets) = fetch_x402_assets(&client, x402_base).await {
        for (caip2_asset, slug) in assets {
            if let Some(entry) = merged.get_mut(&slug) {
                entry.asset = Some(caip2_asset);
            }
        }
    }

    Ok(merged.into_values().collect())
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

/// GET x402 `/discovery/resources` → a map of Quicknode-slug → asset address,
/// derived from the accepted payment offers. Only EVM (`eip155:`) assets carry
/// a stable slug mapping here; the caller treats this as best-effort.
async fn fetch_x402_assets(
    client: &reqwest::Client,
    x402_base: &str,
) -> Result<Vec<(String, String)>, CliError> {
    #[derive(Deserialize)]
    struct Resources {
        items: Vec<ResourceItem>,
    }
    #[derive(Deserialize)]
    struct ResourceItem {
        #[serde(default)]
        accepts: Vec<Accept>,
    }
    #[derive(Deserialize)]
    struct Accept {
        network: String,
        #[serde(default)]
        asset: Option<String>,
    }

    let url = format!("{x402_base}/discovery/resources");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| fetch_err(&url, e))?;
    let body: Resources = resp.json().await.map_err(|e| fetch_err(&url, e))?;

    // Map the CAIP-2 network in each offer back to a Quicknode slug via the
    // static pay_network table, so the asset lands on the right row.
    let mut out = Vec::new();
    for item in body.items {
        for accept in item.accepts {
            if let (Some(asset), Some(slug)) = (
                accept.asset,
                super::pay_network::slug_for_caip2(&accept.network),
            ) {
                out.push((asset, slug));
            }
        }
    }
    Ok(out)
}

#[derive(serde::Serialize)]
struct PayNetworksView(Vec<PayNetworkEntry>);

impl Render for PayNetworksView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        if self.0.is_empty() {
            writeln!(w, "(no payable networks)")?;
            return Ok(());
        }
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["NETWORK", "SCHEMES", "X402 ASSET"]);
        for n in &self.0 {
            t.add_row(vec![
                comfy_table::Cell::new(&n.network),
                comfy_table::Cell::new(n.schemes.join(", ")),
                crate::output::opt_cell(&n.asset),
            ]);
        }
        write_table(w, &t)
    }
}

fn fetch_err(url: &str, e: reqwest::Error) -> CliError {
    CliError::Arg(format!("could not fetch payable networks from {url}: {e}"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
