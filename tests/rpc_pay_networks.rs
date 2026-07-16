//! Integration tests for `qn rpc pay-networks` — the keyless payable-networks
//! discovery list.
//!
//! `--base-url` points both gateway fetches (`/networks` and
//! `/discovery/resources`) at one wiremock host and bypasses the on-disk
//! cache, so the mock server stands in for the x402 + MPP gateways. The
//! in-process harness can't capture stdout, so the merge/asset-mapping render
//! is asserted via a subprocess; the in-process test covers exit codes and the
//! request shape.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mounts the two discovery endpoints the command reads.
async fn mount_discovery(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "networks": ["base-sepolia", "ethereum-mainnet", "solana-devnet"]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/discovery/resources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "x402Version": 2,
            "items": [
                {
                    "accepts": [
                        {
                            "scheme": "exact",
                            "network": "eip155:84532",
                            "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
                        }
                    ]
                }
            ]
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn pay_networks_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_discovery(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "pay-networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn pay_networks_alias_works() {
    let server = MockServer::start().await;
    mount_discovery(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "pay-nets"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn pay_networks_surfaces_fetch_failure() {
    let server = MockServer::start().await;
    // /networks returns 500 — the command should fail with an actionable error.
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["rpc", "pay-networks"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("payable networks") || out.stderr.contains("discovery"),
        "stderr={}",
        out.stderr
    );
}

// Subprocess: assert the merged/enriched table content (stdout).
#[tokio::test]
async fn pay_networks_renders_merged_table() {
    let server = MockServer::start().await;
    mount_discovery(&server).await;

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
            "pay-networks",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // All three slugs present, sorted; base-sepolia carries the x402 asset from
    // the discovery catalog.
    assert!(stdout.contains("base-sepolia"), "{stdout}");
    assert!(stdout.contains("ethereum-mainnet"), "{stdout}");
    assert!(stdout.contains("solana-devnet"), "{stdout}");
    assert!(
        stdout.contains("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
        "asset not mapped onto base-sepolia row: {stdout}"
    );
}
