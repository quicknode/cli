//! Retry behavior: read-only commands retry transient failures with backoff;
//! mutating commands never retry.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn endpoints_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "data": [],
        "pagination": { "total": 0, "limit": 20, "offset": 0 }
    }))
}

#[tokio::test]
async fn read_retries_a_429_then_succeeds() {
    let server = MockServer::start().await;
    // First request 429s; the mock then expires and the 200 mock takes over.
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(endpoints_ok())
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["endpoint", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn read_exhausts_retries_on_persistent_429() {
    let server = MockServer::start().await;
    // --retries 1 → exactly 2 requests (initial + 1 retry), then exit 2.
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(429))
        .expect(2)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["--retries", "1", "endpoint", "list"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}

#[tokio::test]
async fn retries_0_fails_on_first_429() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["--retries", "0", "endpoint", "list"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}

#[tokio::test]
async fn read_does_not_retry_a_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-missing"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["endpoint", "show", "ep-missing"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}

#[tokio::test]
async fn create_is_never_retried_even_on_500() {
    let server = MockServer::start().await;
    // A mutating POST must hit the server exactly once: with no idempotency
    // keys, a retried create could provision (and bill) twice.
    Mock::given(method("POST"))
        .and(path("/v0/endpoints"))
        .and(body_json(
            json!({ "chain": "ethereum", "network": "mainnet" }),
        ))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "create",
            "--chain",
            "ethereum",
            "--network",
            "mainnet",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}
