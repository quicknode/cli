//! `qn micropayments` shares the x402/mpp runners with `qn rpc`.

mod common;

use common::run_qn;
use serde_json::json;
use std::io::Write;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EVM_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const USDC: &str = "0x036CbD53842c5426634e7929541eC2318f3dCF7e";

fn key_file() -> (tempfile::NamedTempFile, String) {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(EVM_KEY.as_bytes()).unwrap();
    f.flush().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    (f, path)
}

fn x402_args<'a>(noun: &'a str, cfg: &'a str, key_path: &'a str, verb: &'a str) -> Vec<&'a str> {
    vec![
        "--config-file",
        cfg,
        noun,
        "x402",
        verb,
        "--payment-key-file",
        key_path,
        "--payment-network",
        "eip155:84532",
        "--payment-asset",
        USDC,
        "--max-amount",
        "10000000",
    ]
}

#[tokio::test]
async fn micropayments_x402_balance_hits_the_same_mocks_as_rpc() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "jwt-test",
            "expiresAt": "2099-01-01T00:00:00Z",
            "accountId": "eip155:84532:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "eip155:84532:0xabc", "credits": 42u64
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "micropayments",
            "x402",
            "balance",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn pay_alias_x402_balance_hits_the_same_mocks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "jwt-test",
            "expiresAt": "2099-01-01T00:00:00Z",
            "accountId": "eip155:84532:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "eip155:84532:0xabc", "credits": 7u64
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "pay",
            "x402",
            "balance",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn micropayments_buy_credits_without_yes_exits_5_and_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    let args = x402_args("micropayments", &cfg, &key_path, "buy-credits");
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}
