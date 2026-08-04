//! Integration tests for keyless payment-gateway discovery commands.

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use common::run_qn;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount the x402 network list.
async fn mount_x402_networks(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "networks": ["base-sepolia", "ethereum-mainnet", "solana-devnet"]
        })))
        .mount(server)
        .await;
}

/// Mount the x402 payment catalog.
async fn mount_x402_supported(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/supported"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "x402Version": 2,
            "accepts": [
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

/// Build an MPP discovery challenge header.
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

/// Mount the MPP discovery endpoints.
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

/// Run the binary with a selected output format.
async fn run_qn_bin(server: &MockServer, fmt: &str, args: &[&str]) -> (String, String, bool) {
    let uri = server.uri();
    let mut argv = vec![
        "--base-url",
        uri.as_str(),
        "--no-input",
        "--no-color",
        "--format",
        fmt,
    ];
    argv.extend(args);
    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .args(&argv)
        .output()
        .unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

#[tokio::test]
async fn x402_supported_networks_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_x402_networks(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "x402", "supported-networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    let out = run_qn(&server.uri(), &["rpc", "x402", "networks"]).await;
    assert_eq!(out.exit_code, 0, "alias failed: stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_supported_payments_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_x402_supported(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "x402", "supported-payments"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    let out = run_qn(&server.uri(), &["rpc", "x402", "payments"]).await;
    assert_eq!(out.exit_code, 0, "alias failed: stderr={}", out.stderr);
}

#[tokio::test]
async fn mpp_supported_networks_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_mpp(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "mpp", "supported-networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn mpp_supported_payments_fetches_and_exits_zero() {
    let server = MockServer::start().await;
    mount_mpp(&server).await;

    let out = run_qn(&server.uri(), &["rpc", "mpp", "supported-payments"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn supported_networks_surfaces_fetch_failure() {
    let server = MockServer::start().await;
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

#[tokio::test]
async fn supported_payments_surfaces_fetch_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/supported"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["rpc", "x402", "supported-payments"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("discovery") || out.stderr.contains("gateway catalog"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn x402_supported_networks_json_is_a_slug_array() {
    let server = MockServer::start().await;
    mount_x402_networks(&server).await;

    let (stdout, stderr, ok) =
        run_qn_bin(&server, "json", &["rpc", "x402", "supported-networks"]).await;
    assert!(ok, "stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(
        v,
        json!(["base-sepolia", "ethereum-mainnet", "solana-devnet"])
    );
}

#[tokio::test]
async fn x402_supported_payments_json_is_an_options_array() {
    let server = MockServer::start().await;
    mount_x402_supported(&server).await;

    let (stdout, stderr, ok) =
        run_qn_bin(&server, "json", &["rpc", "x402", "supported-payments"]).await;
    assert!(ok, "stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(
        v,
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

#[tokio::test]
async fn mpp_supported_payments_renders_challenge_options() {
    let server = MockServer::start().await;
    mount_mpp(&server).await;

    let (stdout, stderr, ok) =
        run_qn_bin(&server, "table", &["rpc", "mpp", "supported-payments"]).await;
    assert!(ok, "stderr={stderr}");

    assert!(stdout.contains("tempo-testnet"), "{stdout}");
    assert!(stdout.contains("pathUSD"), "{stdout}");
    assert!(stdout.contains("solana-mainnet"), "{stdout}");
    assert!(stdout.contains("USDC"), "{stdout}");
    assert!(
        stdout.contains("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
        "{stdout}"
    );
}
