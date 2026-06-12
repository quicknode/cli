//! Snapshot tests for human-readable table output, rendered by the real
//! binary against a wiremock server.
//!
//! Unlike `output_snapshots.rs` (which pins layout via re-declared renderers),
//! these run `qn` as a subprocess so the snapshot covers the actual
//! decode-and-render path for each command.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount `body` at `GET url_path`, run `qn --format table <args>` against the
/// mock server, and return stdout. Panics (with stderr) on non-zero exit.
async fn table_stdout(url_path: &str, body: serde_json::Value, args: &[&str]) -> String {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let uri = server.uri();
    let mut argv = vec![
        "--api-key",
        "test",
        "--base-url",
        uri.as_str(),
        "--no-input",
        "--format",
        "table",
    ];
    argv.extend(args);
    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .env_remove("HOME")
        .env("HOME", std::env::temp_dir())
        .args(&argv)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn endpoint_show_full_table() {
    let body = serde_json::json!({
        "data": {
            "id": "ep-1",
            "label": null,
            "status": "active",
            "chain": "eth",
            "network": "mainnet",
            "http_url": "https://ep-1.example",
            "wss_url": "wss://ep-1.example",
            "security": {
                "options": {
                    "tokens": true,
                    "jwts": false,
                    "domainMasks": false,
                    "ips": false,
                    "referrers": false,
                    "requestFilters": false,
                    "ipCustomHeader": { "value": null }
                },
                "tokens": [
                    { "id": "tok-1", "token": "0xabc" }
                ],
                "jwts": null,
                "referrers": null,
                "domain_masks": null,
                "ips": null,
                "request_filters": null
            },
            "rate_limits": {
                "rate_limit_by_ip": false,
                "account": -1,
                "rps": -1,
                "rpm": -1,
                "rpd": -1
            },
            "tags": [],
            "is_multichain": false
        }
    });
    let out = table_stdout("/v0/endpoints/ep-1", body, &["endpoint", "show", "ep-1"]).await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_security_show_all_sections() {
    let body = serde_json::json!({
        "data": {
            "options": {
                "tokens": true,
                "jwts": true,
                "domainMasks": true,
                "ips": true,
                "referrers": true,
                "requestFilters": true,
                "ipCustomHeader": { "value": "x-real-ip" }
            },
            "tokens": [
                { "id": "tok-1", "token": "0xabc" },
                { "id": "tok-2", "token": "0xdef" }
            ],
            "jwts": [
                { "id": "jwt-1", "public_key": "pk", "kid": "kid-1", "name": "ci" }
            ],
            "referrers": [
                { "id": "ref-1", "referrer": "https://app.example.com" }
            ],
            "domain_masks": [
                { "id": "dm-1", "domain": "*.example.com" }
            ],
            "ips": [
                { "id": "ip-1", "ip": "203.0.113.7" }
            ],
            "request_filters": [
                { "id": "rf-1", "method": ["eth_blockNumber", "eth_call"] }
            ]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/security",
        body,
        &["endpoint", "security", "show", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_security_show_single_token_omits_empty_sections() {
    let body = serde_json::json!({
        "data": {
            "options": {
                "tokens": true,
                "jwts": false,
                "domainMasks": false,
                "ips": false,
                "referrers": false,
                "requestFilters": false,
                "ipCustomHeader": { "value": null }
            },
            "tokens": [
                { "id": "tok-1", "token": "0xabc" }
            ],
            "jwts": null,
            "referrers": null,
            "domain_masks": null,
            "ips": null,
            "request_filters": null
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/security",
        body,
        &["endpoint", "security", "show", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_show_minimal_table_omits_security_and_rate_limit_rows() {
    let body = serde_json::json!({
        "data": {
            "id": "ep-1",
            "chain": "solana",
            "network": "mainnet",
            "http_url": "https://ep-1.example",
            "tags": []
        }
    });
    let out = table_stdout("/v0/endpoints/ep-1", body, &["endpoint", "show", "ep-1"]).await;
    insta::assert_snapshot!(out);
}
