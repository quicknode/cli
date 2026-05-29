//! Stage-3 admin-surface integration tests (tags, teams, usage, metrics, chain,
//! billing, bulk).

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tag_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "tags": [{ "id": 1, "label": "prod", "usage_count": 3 }] }
        })))
        .mount(&server)
        .await;
    assert_eq!(run_qn(&server.uri(), &["tag", "list"]).await.exit_code, 0);
}

#[tokio::test]
async fn tag_rename_sends_label() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/tags/42"))
        .and(body_json(json!({ "label": "staging" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": 42, "label": "staging", "usage_count": 0 }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["tag", "rename", "42", "staging"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn tag_delete_needs_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/tags/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "success": true }
        })))
        .mount(&server)
        .await;
    let no = run_qn(&server.uri(), &["tag", "delete", "42"]).await;
    assert_eq!(no.exit_code, 5);
    let yes = run_qn(&server.uri(), &["tag", "delete", "42", "--yes"]).await;
    assert_eq!(yes.exit_code, 0, "stderr={}", yes.stderr);
}

#[tokio::test]
async fn team_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": 7, "name": "Payments", "members_count": 4 }]
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["team", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn team_create_sends_name() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/teams"))
        .and(body_json(json!({ "name": "Ops" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": 8, "name": "Ops" }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["team", "create", "--name", "Ops"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn team_member_invite() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/teams/7/members"))
        .and(body_json(json!({ "email": "a@x.io", "role": "viewer" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": 99, "email": "a@x.io" }
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "team", "member", "invite", "7", "--email", "a@x.io", "--role", "viewer",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_summary() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "credits_used": 1000,
                "credits_remaining": 9000,
                "limit": 10000,
                "start_time": 0,
                "end_time": 0
            }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["usage", "summary"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn usage_by_endpoint_with_range() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/usage/rpc/by-endpoint"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "endpoints": [] }
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["usage", "by-endpoint", "--from", "7d", "--to", "now"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn chain_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/chains"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "slug": "ethereum", "networks": [{ "slug": "mainnet", "name": "Mainnet" }] }
            ]
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["chain", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn metrics_account_with_percentile() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/metrics"))
        .and(query_param("period", "day"))
        .and(query_param("metric", "credits_over_time"))
        .and(query_param("percentile", "p95"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "metrics",
            "account",
            "--period",
            "day",
            "--metric",
            "credits_over_time",
            "--percentile",
            "p95",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn billing_invoices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/billing/invoices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "invoices": [] }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["billing", "invoices"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn bulk_status_paused() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/bulk/status"))
        .and(body_json(
            json!({ "ids": ["ep-1", "ep-2"], "status": "paused" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "total": 2, "updated_count": 2, "failed_count": 0, "results": [] }
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["bulk", "status", "--status", "paused", "ep-1", "ep-2"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn bulk_tag_add() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/bulk/tags"))
        .and(body_json(json!({ "ids": ["ep-1"], "label": "prod" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "total": 1, "updated_count": 1, "failed_count": 0, "results": [], "tag": { "tag_id": 1, "label": "prod" } }
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["bulk", "tag", "add", "--label", "prod", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}
