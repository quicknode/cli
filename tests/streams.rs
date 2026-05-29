//! Integration tests for `qn stream …`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn stream_payload(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": "test-stream",
        "status": "active",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "sequence": 0,
        "network": "ethereum-mainnet",
        "dataset": "block",
        "region": "usa_east",
        "start_range": 0,
        "end_range": -1,
    })
}

#[tokio::test]
async fn list_streams() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/streams/rest/v1/streams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [stream_payload("s-1")],
            "pageInfo": { "limit": 20, "offset": 0, "total": 1 }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["stream", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn create_stream_webhook_destination() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/streams/rest/v1/streams"))
        .and(body_partial_json(json!({
            "name": "s1",
            "network": "ethereum-mainnet",
            "dataset": "block",
            "start_range": 100,
            "end_range": -1,
            "region": "usa_east",
            "destination": "webhook",
            "destination_attributes": { "url": "https://hook.example/x" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(stream_payload("s-new")))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "stream",
            "create",
            "--name",
            "s1",
            "--network",
            "ethereum-mainnet",
            "--dataset",
            "block",
            "--start",
            "100",
            "--end",
            "-1",
            "--region",
            "usa-east",
            "--webhook",
            "https://hook.example/x",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn show_stream() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/streams/rest/v1/streams/s-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(stream_payload("s-1")))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["stream", "show", "s-1"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn activate_pause_delete_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/streams/rest/v1/streams/s-1/activate"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/streams/rest/v1/streams/s-1/pause"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/streams/rest/v1/streams/s-1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    assert_eq!(
        run_qn(&server.uri(), &["stream", "activate", "s-1"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["stream", "pause", "s-1"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["stream", "delete", "s-1", "--yes"])
            .await
            .exit_code,
        0
    );
}

#[tokio::test]
async fn delete_all_streams_needs_double_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/streams/rest/v1/streams"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // Single --yes is not enough for severe.
    let one_yes = run_qn(&server.uri(), &["stream", "delete-all", "--yes"]).await;
    assert_eq!(one_yes.exit_code, 5, "stderr={}", one_yes.stderr);

    // --yes --yes proceeds.
    let two_yes = run_qn(&server.uri(), &["stream", "delete-all", "--yes", "--yes"]).await;
    assert_eq!(two_yes.exit_code, 0, "stderr={}", two_yes.stderr);
}

#[tokio::test]
async fn enabled_count_prints_total() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/streams/rest/v1/streams/enabled_count"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "total": 7 })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["stream", "enabled-count"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn test_filter_sends_base64_filter() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/streams/rest/v1/streams/test_filter"))
        .and(body_partial_json(json!({
            "network": "ethereum-mainnet",
            "dataset": "block",
            "block": "1234567"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "\"hi\"",
            "logs": []
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "stream",
            "test-filter",
            "--network",
            "ethereum-mainnet",
            "--dataset",
            "block",
            "--block",
            "1234567",
            "--filter",
            "function main(d){return d;}",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}
