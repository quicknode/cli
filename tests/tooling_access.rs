//! Integration tests for `qn tooling-access {status,enable,disable}`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn status_returns_enabled() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "enabled": true,
                "endpoint_url": "https://tooling-access-abc123.quiknode.pro",
                "enabled_at": "2026-06-23T20:30:00.000Z"
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["tooling-access", "status"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn enable_sends_enabled_true() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/tooling-access"))
        .and(body_json(json!({ "enabled": true })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": true, "endpoint_url": "https://x.quiknode.pro" },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["tooling-access", "enable"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn disable_sends_enabled_false() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/tooling-access"))
        .and(body_json(json!({ "enabled": false })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": false },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["tooling-access", "disable"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn enable_surfaces_ineligible_plan_error() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "data": null,
            "error": "The legacy billing plan for your account doesn't support tooling access."
        })))
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["tooling-access", "enable"]).await;
    // Api error → exit 2.
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}
