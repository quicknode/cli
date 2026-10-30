//! Integration tests for `qn transaction …`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FROM: &str = "2026-01-01T00:00:00Z";
const TO: &str = "2026-01-02T00:00:00Z";

fn list_body() -> serde_json::Value {
    json!({
        "data": {
            "from": FROM,
            "to": TO,
            "transactions": [{
                "network": "ethereum-mainnet",
                "transaction_hash": "0xabc",
                "first_submitted_at": "2026-01-01T10:00:00Z",
                "last_submitted_at": "2026-01-01T10:05:00Z",
                "attempt_count": 2,
                "endpoint_ids": [123, null],
                "methods": ["eth_sendRawTransaction"],
                "archive": false,
                "height": "9007199254740993",
                "state": "succeeded"
            }],
            "next_cursor": "abc"
        },
        "error": null
    })
}

fn detail_body(with_payloads: bool) -> serde_json::Value {
    let payloads = if with_payloads {
        json!({ "request": "{\"method\":\"eth_sendRawTransaction\"}", "response": "not json" })
    } else {
        serde_json::Value::Null
    };
    json!({
        "data": {
            "exists": true,
            "data": {
                "network": "ethereum-mainnet",
                "transaction_hash": "0xabc",
                "first_submitted_at": "2026-01-01T10:00:00Z",
                "last_submitted_at": "2026-01-01T10:05:00Z",
                "state": "succeeded",
                "attempt_count": 1,
                "endpoint_ids": [123],
                "methods": ["eth_sendRawTransaction"],
                "archive": false,
                "height": "9007199254740993",
                "attempts": [{
                    "submitted_at": "2026-01-01T10:00:00Z",
                    "endpoint_id": 123,
                    "method": "eth_sendRawTransaction",
                    "request_id": "req-1",
                    "payloads": payloads
                }]
            }
        },
        "error": null
    })
}

#[tokio::test]
async fn transaction_list_sends_window_and_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions"))
        .and(query_param("from", FROM))
        .and(query_param("to", TO))
        .and(query_param("state", "failed"))
        .and(query_param("network", "ethereum-mainnet"))
        .and(query_param("method", "eth_sendRawTransaction"))
        .and(query_param("transaction_hash", "0xabc"))
        .and(query_param("endpoints[]", "123"))
        .and(query_param("endpoints[]", "456"))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "transaction",
            "list",
            "--from",
            FROM,
            "--to",
            TO,
            "--state",
            "failed",
            "--network",
            "ethereum-mainnet",
            "--method",
            "eth_sendRawTransaction",
            "--hash",
            "0xabc",
            "--endpoint",
            "123",
            "--endpoint",
            "456",
            "--limit",
            "50",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_list_cursor_travels_alone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions"))
        .and(query_param("cursor", "abc"))
        .and(query_param_is_missing("from"))
        .and(query_param_is_missing("to"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["tx", "ls", "--cursor", "abc"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_list_rejects_cursor_with_a_filter() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "transaction",
            "list",
            "--cursor",
            "abc",
            "--state",
            "failed",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_list_requires_from_or_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["transaction", "list"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("--from"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_list_rejects_unknown_state() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["transaction", "list", "--from", "1h", "--state", "pending"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_show_requests_payloads_only_when_asked() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions/ethereum-mainnet/0xabc"))
        .and(query_param("include_payloads", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(true)))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "transaction",
            "show",
            "ethereum-mainnet",
            "0xabc",
            "--payloads",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_show_omits_include_payloads_by_default() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions/ethereum-mainnet/0xabc"))
        .and(query_param_is_missing("include_payloads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(false)))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["transaction", "show", "ethereum-mainnet", "0xabc"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_show_not_retained_exits_zero() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions/ethereum-mainnet/0xdead"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "exists": false, "data": null }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["transaction", "show", "ethereum-mainnet", "0xdead"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn transaction_show_403_renders_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/transactions/ethereum-mainnet/0xabc"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["transaction", "show", "ethereum-mainnet", "0xabc"],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("forbidden"), "stderr={}", out.stderr);
}
