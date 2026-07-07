//! Stage-3 admin-surface integration tests (endpoint tags/bulk, teams, usage,
//! metrics, chain, billing).

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
    assert_eq!(
        run_qn(&server.uri(), &["endpoint", "tag", "list"])
            .await
            .exit_code,
        0
    );
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
    let out = run_qn(
        &server.uri(),
        &["endpoint", "tag", "rename", "42", "staging"],
    )
    .await;
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
    let no = run_qn(&server.uri(), &["endpoint", "tag", "delete", "42"]).await;
    assert_eq!(no.exit_code, 5);
    let yes = run_qn(&server.uri(), &["endpoint", "tag", "delete", "42", "--yes"]).await;
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
async fn chain_credits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/api-credits/ethereum"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "method": "eth_chainId", "credits": 20 },
                { "method": "eth_call", "credits": 20 }
            ]
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["chain", "credits", "ethereum"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn chain_credits_unknown_slug_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/api-credits/not-a-chain"))
        .respond_with(
            ResponseTemplate::new(404).set_body_string("{\"message\":\"chain not found\"}"),
        )
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["chain", "credits", "not-a-chain"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("not found"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn auth_whoami_shows_account() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/account/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 12345,
                "name": "Acme Inc",
                "created_at": "2024-01-01T00:00:00Z",
                "billing_version": "v6",
                "subscription": { "plan_name": "Build", "status": "active", "interval": "monthly" }
            }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["auth", "whoami"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn auth_whoami_unauthorized_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/account/info"))
        .respond_with(ResponseTemplate::new(401).set_body_string("{\"message\":\"unauthorized\"}"))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["auth", "whoami"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
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
        &["endpoint", "bulk", "pause", "ep-1", "ep-2", "--yes"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn bulk_pause_without_yes_needs_confirmation_and_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/bulk/status"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["endpoint", "bulk", "pause", "ep-1"]).await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn bulk_resume_does_not_need_confirmation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/bulk/status"))
        .and(body_json(json!({ "ids": ["ep-1"], "status": "active" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "total": 1, "updated_count": 1, "failed_count": 0, "results": [] }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["endpoint", "bulk", "resume", "ep-1"]).await;
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
        &["endpoint", "bulk", "tag", "add", "--label", "prod", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn team_set_endpoints_no_ids_fails_before_api_call() {
    let server = MockServer::start().await;
    let out = run_qn(&server.uri(), &["team", "set-endpoints", "7"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("endpoint id"), "stderr={}", out.stderr);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}
