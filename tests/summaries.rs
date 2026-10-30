//! Integration tests for the windowed, cursor-paginated summaries:
//! `usage credits|rpc|streams|webhooks`, `metrics traffic|errors`, and
//! `endpoint errors`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FROM: &str = "2026-01-01T00:00:00Z";
const TO: &str = "2026-01-02T00:00:00Z";

fn rpc_body() -> serde_json::Value {
    json!({
        "data": {
            "from": FROM,
            "to": TO,
            "totals": { "requests": 10, "bytes": 2048, "rpc_credits": 8, "data_credits": 2, "credits": 10 },
            "breakdown": [
                { "method": "eth_call", "endpoint": 123, "requests": 7, "bytes": 1024, "rpc_credits": 6, "data_credits": 1, "credits": 7 },
                { "method": null, "endpoint": 456, "requests": null, "bytes": 1024, "rpc_credits": 2, "data_credits": 1, "credits": 3 }
            ],
            "next_cursor": "abc"
        },
        "error": null
    })
}

fn errors_body() -> serde_json::Value {
    json!({
        "data": {
            "from": FROM,
            "to": TO,
            "totals": { "rpc_errors": 5, "http_errors": 2 },
            "breakdown": [
                { "kind": "rpc", "count": 5, "code": "-32000" },
                { "kind": "rpc", "count": 0.5, "code": null }
            ]
        },
        "error": null
    })
}

#[tokio::test]
async fn usage_credits_sends_from_and_to() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/credits/summary"))
        .and(query_param("from", FROM))
        .and(query_param("to", TO))
        .and(query_param_is_missing("cursor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "from": FROM, "to": TO, "totals": { "credits": 1200.5 } },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["usage", "credits", "--from", FROM, "--to", TO],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_credits_requires_from_or_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/credits/summary"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["usage", "credits"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("--from"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_rpc_sends_group_by_filters_and_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc/summary"))
        .and(query_param("from", FROM))
        .and(query_param("group_by[]", "method"))
        .and(query_param("group_by[]", "endpoint"))
        .and(query_param("chains[]", "ethereum"))
        .and(query_param("methods[]", "eth_call"))
        .and(query_param("tag_ids[]", "7"))
        .and(query_param("limit", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rpc_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "usage",
            "rpc",
            "--from",
            FROM,
            "--group-by",
            "method,endpoint",
            "--chain",
            "ethereum",
            "--method",
            "eth_call",
            "--tag-id",
            "7",
            "--limit",
            "20",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_rpc_cursor_travels_alone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc/summary"))
        .and(query_param("cursor", "abc"))
        .and(query_param_is_missing("from"))
        .and(query_param_is_missing("to"))
        .and(query_param_is_missing("limit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rpc_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["usage", "rpc", "--cursor", "abc"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_rpc_rejects_cursor_combined_with_window_or_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc/summary"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let with_from = run_qn(
        &server.uri(),
        &["usage", "rpc", "--cursor", "abc", "--from", "1h"],
    )
    .await;
    assert_eq!(with_from.exit_code, 1, "stderr={}", with_from.stderr);
    let with_filter = run_qn(
        &server.uri(),
        &["usage", "rpc", "--cursor", "abc", "--chain", "ethereum"],
    )
    .await;
    assert_eq!(with_filter.exit_code, 1, "stderr={}", with_filter.stderr);
    assert!(
        with_filter.stderr.contains("--cursor"),
        "stderr={}",
        with_filter.stderr
    );
}

#[tokio::test]
async fn usage_rpc_rejects_unknown_group_by() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc/summary"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["usage", "rpc", "--from", "1h", "--group-by", "stream"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_rpc_limit_out_of_range_fails_before_api_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc/summary"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["usage", "rpc", "--from", "1h", "--limit", "1001"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_streams_sends_group_by_and_stream_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/streams/summary"))
        .and(query_param("from", FROM))
        .and(query_param("group_by[]", "stream"))
        .and(query_param("stream_id", "st-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "from": FROM, "to": TO,
                "totals": { "blocks": 100, "bytes": 5000, "block_credits": 200 },
                "breakdown": [{ "stream": "st-1", "blocks": 100, "bytes": 5000, "block_credits": 200 }]
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "usage",
            "streams",
            "--from",
            FROM,
            "--group-by",
            "stream",
            "--stream-id",
            "st-1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_webhooks_sends_webhook_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/webhooks/summary"))
        .and(query_param("from", FROM))
        .and(query_param("webhook_id", "wh-1"))
        .and(query_param_is_missing("group_by[]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "from": FROM, "to": TO,
                "totals": { "deliveries": 40, "credits": 4 }
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["usage", "webhooks", "--from", FROM, "--webhook-id", "wh-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn metrics_traffic_sends_group_by_and_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/metrics/traffic/summary"))
        .and(query_param("from", FROM))
        .and(query_param("group_by[]", "endpoint"))
        .and(query_param("endpoint_names[]", "ep-1"))
        .and(query_param("networks[]", "mainnet"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "from": FROM, "to": TO,
                "totals": { "successful_requests": 9, "bytes": 1, "request_credits": 2, "bytes_credits": 3, "credits": 5, "active_methods": 4 },
                "breakdown": [{ "endpoint": 123, "successful_requests": 9, "bytes": 1, "request_credits": 2, "bytes_credits": 3, "credits": 5, "active_methods": 4 }]
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "metrics",
            "traffic",
            "--from",
            FROM,
            "--group-by",
            "endpoint",
            "--endpoint-name",
            "ep-1",
            "--network",
            "mainnet",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn metrics_errors_sends_kind_endpoints_and_codes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/metrics/errors/summary"))
        .and(query_param("from", FROM))
        .and(query_param("kind", "rpc"))
        .and(query_param("endpoints[]", "123"))
        .and(query_param("endpoints[]", "456"))
        .and(query_param("codes[]", "-32000"))
        .and(query_param("group_by[]", "code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(errors_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "metrics",
            "errors",
            "--from",
            FROM,
            "--kind",
            "rpc",
            "--endpoint",
            "123",
            "--endpoint",
            "456",
            "--code",
            "-32000",
            "--group-by",
            "code",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn metrics_errors_403_renders_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/metrics/errors/summary"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["metrics", "errors", "--from", "1h"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("forbidden"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_errors_hits_the_endpoint_route() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/metrics/errors/summary"))
        .and(query_param("from", FROM))
        .and(query_param("kind", "http"))
        .and(query_param("status_codes[]", "500"))
        .and(query_param("group_by[]", "code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(errors_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "errors",
            "ep-1",
            "--from",
            FROM,
            "--kind",
            "http",
            "--status-code",
            "500",
            "--group-by",
            "code",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_errors_rejects_account_only_group_by() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/metrics/errors/summary"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "errors",
            "ep-1",
            "--from",
            "1h",
            "--group-by",
            "endpoint",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}
