//! Integration tests for `qn rpc x402 supported-networks` and
//! `qn rpc mpp supported-networks` — the keyless per-gateway discovery
//! catalogs (callable networks + accepted currencies).
//!
//! `--base-url` points the gateway fetches at one wiremock host and bypasses
//! the on-disk cache, so the mock server stands in for the gateway. The
//! in-process harness can't capture stdout, so rendered output (tables, JSON
//! shape, the best-effort warning) is asserted via a subprocess; the
//! in-process tests cover exit codes.

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use common::run_qn;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mounts the two x402 discovery endpoints (`/networks` + `/supported`).
async fn mount_x402(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "networks": ["base-sepolia", "ethereum-mainnet", "solana-devnet"]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/supported"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "x402Version": 2,
            "accepts": [
                // Two offers for the same (network, asset): a plain token
                // offer and a Circle Gateway variant (verifyingContract) whose
                // name is an EIP-712 domain — must dedupe to one USDC row.
                {
                    "scheme": "exact",
                    "network": "eip155:84532",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "extra": {"name": "USDC", "version": "2"}
                },
                {
                    "scheme": "exact",
                    "network": "eip155:84532",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "extra": {
                        "name": "GatewayWalletBatched",
                        "version": "1",
                        "verifyingContract": "0x0000000000000000000000000000000000000001"
                    }
                },
                // A network outside the slug table renders as its raw CAIP-2
                // id, with the offer's token name.
                {
                    "scheme": "exact",
                    "network": "eip155:999999",
                    "asset": "0x00000000000000000000000000000000000000aa",
                    "extra": {"name": "Fake Dollar", "version": "1"}
                }
            ]
        })))
        .mount(server)
        .await;
}

/// Builds an MPP `WWW-Authenticate` header with one Tempo-testnet pathUSD
/// challenge and one Solana-mainnet USDC challenge.
fn mpp_challenge_header() -> String {
    let tempo = URL_SAFE_NO_PAD.encode(
        json!({
            "amount": "1000",
            "currency": "0x20c0000000000000000000000000000000000000",
            "methodDetails": {"chainId": 42431, "feePayer": true},
            "recipient": "0x0000000000000000000000000000000000000002"
        })
        .to_string(),
    );
    let solana = URL_SAFE_NO_PAD.encode(
        json!({
            "amount": "0.001",
            "currency": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "methodDetails": {"decimals": 6, "network": "mainnet-beta"},
            "recipient": "Recipient1111111111111111111111111111111111"
        })
        .to_string(),
    );
    format!(
        "Payment id=\"a\", realm=\"mock\", method=\"tempo\", intent=\"charge\", \
         request=\"{tempo}\", description=\"Quicknode RPC request\", \
         Payment id=\"b\", realm=\"mock\", method=\"solana\", intent=\"charge\", \
         request=\"{solana}\", description=\"Quicknode RPC request\""
    )
}

/// Mounts the MPP discovery surface: `/networks` plus the keyless 402 probe
/// against the first listed network.
async fn mount_mpp(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "networks": ["base-sepolia", "tempo-testnet"]
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(
            ResponseTemplate::new(402)
                .insert_header("www-authenticate", mpp_challenge_header().as_str()),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn x402_supported_networks_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_x402(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "x402", "supported-networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_supported_networks_alias_works() {
    let server = MockServer::start().await;
    mount_x402(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "x402", "networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn mpp_supported_networks_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_mpp(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "mpp", "supported-networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn supported_networks_surfaces_callable_fetch_failure() {
    let server = MockServer::start().await;
    // /networks returns 500 — the command should fail with an actionable error.
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["rpc", "x402", "supported-networks"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("discovery") || out.stderr.contains("gateway catalog"),
        "stderr={}",
        out.stderr
    );
}

/// A currencies fetch failure is best-effort: exit 0, callable table renders,
/// a warning lands on stderr, and the JSON payments field is null.
#[tokio::test]
async fn x402_currencies_failure_is_best_effort() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "networks": ["base-sepolia"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/supported"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .args([
            "--base-url",
            &server.uri(),
            "--no-input",
            "--no-color",
            "--format",
            "json",
            "rpc",
            "x402",
            "supported-networks",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr={stderr}");
    assert!(
        stderr.contains("could not fetch accepted currencies"),
        "stderr={stderr}"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(v["callable"], json!(["base-sepolia"]));
    assert!(v["payments"].is_null(), "{v}");
}

// Subprocess: assert the two-section x402 render and the JSON shape.
#[tokio::test]
async fn x402_supported_networks_renders_two_sections() {
    let server = MockServer::start().await;
    mount_x402(&server).await;

    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .args([
            "--base-url",
            &server.uri(),
            "--no-input",
            "--no-color",
            "--format",
            "table",
            "rpc",
            "x402",
            "supported-networks",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(stdout.contains("Callable networks"), "{stdout}");
    assert!(stdout.contains("Accepted currencies"), "{stdout}");
    assert!(stdout.contains("ethereum-mainnet"), "{stdout}");
    // The payment network maps to its slug, the two offers dedupe to one row,
    // and the known address resolves to its symbol.
    assert_eq!(stdout.matches("base-sepolia").count(), 2, "{stdout}");
    assert!(
        stdout.contains("USDC") && !stdout.contains("GatewayWalletBatched"),
        "{stdout}"
    );
    // The unknown payment network falls back to its raw CAIP-2 id with the
    // offer's token name.
    assert!(stdout.contains("eip155:999999"), "{stdout}");
    assert!(stdout.contains("Fake Dollar"), "{stdout}");
}

#[tokio::test]
async fn x402_supported_networks_json_shape() {
    let server = MockServer::start().await;
    mount_x402(&server).await;

    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .args([
            "--base-url",
            &server.uri(),
            "--no-input",
            "--format",
            "json",
            "rpc",
            "x402",
            "supported-networks",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(
        v["callable"],
        json!(["base-sepolia", "ethereum-mainnet", "solana-devnet"])
    );
    assert_eq!(
        v["payments"],
        json!([
            {
                "network": "base-sepolia",
                "asset": "USDC",
                "address": "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
            },
            {
                "network": "eip155:999999",
                "asset": "Fake Dollar",
                "address": "0x00000000000000000000000000000000000000aa"
            }
        ])
    );
}

// Subprocess: assert the MPP challenge-derived currencies render.
#[tokio::test]
async fn mpp_supported_networks_renders_challenge_currencies() {
    let server = MockServer::start().await;
    mount_mpp(&server).await;

    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .args([
            "--base-url",
            &server.uri(),
            "--no-input",
            "--no-color",
            "--format",
            "table",
            "rpc",
            "mpp",
            "supported-networks",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Tempo testnet resolves to its slug and the enshrined token to pathUSD;
    // the Solana challenge maps to solana-mainnet USDC.
    assert!(stdout.contains("tempo-testnet"), "{stdout}");
    assert!(stdout.contains("pathUSD"), "{stdout}");
    assert!(stdout.contains("solana-mainnet"), "{stdout}");
    assert!(stdout.contains("USDC"), "{stdout}");
    assert!(
        stdout.contains("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
        "{stdout}"
    );
}
