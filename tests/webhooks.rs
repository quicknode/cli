//! Integration tests for `qn webhook …`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn webhook_payload(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": "wh-test",
        "status": "active",
        "network": "ethereum-mainnet",
        "created_at": "2026-01-01T00:00:00Z",
        "template_id": "evmWalletFilter"
    })
}

#[tokio::test]
async fn list_webhooks() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/webhooks/rest/v1/webhooks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [webhook_payload("wh-1")],
            "pageInfo": { "limit": 20, "offset": 0, "total": 1 }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["webhook", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn create_webhook_evm_wallet() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhooks/rest/v1/webhooks/template/evmWalletFilter"))
        .and(body_partial_json(json!({
            "name": "w1",
            "network": "ethereum-mainnet",
            "destination_attributes": { "url": "https://hook.example/x" },
            "templateId": "evmWalletFilter",
            "templateArgs": { "wallets": ["0xabc"] }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(webhook_payload("wh-1")))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "webhook",
            "create",
            "--name",
            "w1",
            "--network",
            "ethereum-mainnet",
            "--url",
            "https://hook.example/x",
            "--template",
            "evm-wallet",
            "--wallet",
            "0xabc",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn create_webhook_evm_contract_events() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/webhooks/rest/v1/webhooks/template/evmContractEvents",
        ))
        .and(body_partial_json(json!({
            "templateId": "evmContractEvents",
            "templateArgs": { "contracts": ["0xc1"], "eventHashes": ["0xtopic"] }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(webhook_payload("wh-c")))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "webhook",
            "create",
            "--name",
            "w2",
            "--network",
            "ethereum-mainnet",
            "--url",
            "https://hook.example/y",
            "--template",
            "evm-contract-events",
            "--contract",
            "0xc1",
            "--event-hash",
            "0xtopic",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn pause_activate_delete_webhook() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhooks/rest/v1/webhooks/wh-1/pause"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/webhooks/rest/v1/webhooks/wh-1/activate"))
        .and(body_partial_json(json!({ "startFrom": "latest" })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/webhooks/rest/v1/webhooks/wh-1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    assert_eq!(
        run_qn(&server.uri(), &["webhook", "pause", "wh-1"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["webhook", "activate", "wh-1"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["webhook", "delete", "wh-1", "--yes"])
            .await
            .exit_code,
        0
    );
}

#[tokio::test]
async fn delete_all_is_not_a_command() {
    // Account-wide wipes are deliberately not offered by the CLI.
    let server = MockServer::start().await;
    let out = run_qn(&server.uri(), &["webhook", "delete-all", "-y", "-y"]).await;
    assert_ne!(out.exit_code, 0);
}

#[tokio::test]
async fn webhook_enabled_count() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/webhooks/rest/v1/webhooks/enabled_count"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "total": 9 })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["webhook", "enabled-count"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn webhook_create_missing_wallets_errors() {
    let server = MockServer::start().await;
    // No mock — the request shouldn't even fire because validation fails.
    let _ = server;
    let out = run_qn(
        "http://127.0.0.1:1", // unreachable
        &[
            "webhook",
            "create",
            "--name",
            "x",
            "--network",
            "ethereum-mainnet",
            "--url",
            "https://hook.example",
            "--template",
            "evm-wallet",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("--wallet"), "stderr={}", out.stderr);
}

// ---- 400 error rendering ---- //

/// A close-to-real NestJS validator 400 from the webhooks service.
fn nestjs_network_400() -> serde_json::Value {
    json!({
        "statusCode": 400,
        "timestamp": "2026-06-04T14:45:19.326Z",
        "path": "/webhooks/rest/v1/webhooks/template/evmWalletFilter",
        "message": {
            "message": [
                "network must be one of the following values: ethereum-mainnet, ethereum-sepolia, solana-mainnet, solana-devnet, base-mainnet, base-sepolia, polygon-mainnet, polygon-amoy"
            ],
            "error": "Bad Request",
            "statusCode": 400
        }
    })
}

#[tokio::test]
async fn create_webhook_400_renders_bullets_with_typo_suggestion() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhooks/rest/v1/webhooks/template/evmWalletFilter"))
        .respond_with(ResponseTemplate::new(400).set_body_json(nestjs_network_400()))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "webhook",
            "create",
            "--name",
            "w1",
            "--network",
            "ethereum-mainnetsds",
            "--url",
            "https://hook.example/x",
            "--template",
            "evm-wallet",
            "--wallet",
            "0xabc",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("invalid request"),
        "stderr={}",
        out.stderr
    );
    assert!(
        out.stderr.contains("network must be one of"),
        "stderr={}",
        out.stderr
    );
    assert!(
        out.stderr.contains("did you mean 'ethereum-mainnet'"),
        "stderr={}",
        out.stderr
    );
    assert!(
        out.stderr.contains("qn chain list"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn create_webhook_400_far_typo_no_suggestion() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhooks/rest/v1/webhooks/template/evmWalletFilter"))
        .respond_with(ResponseTemplate::new(400).set_body_json(nestjs_network_400()))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "webhook",
            "create",
            "--name",
            "w1",
            "--network",
            "sfjla",
            "--url",
            "https://hook.example/x",
            "--template",
            "evm-wallet",
            "--wallet",
            "0xabc",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("network must be one of"),
        "stderr={}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("did you mean"),
        "should not suggest for 'sfjla'; stderr={}",
        out.stderr
    );
}
