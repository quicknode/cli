//! Integration tests for `qn activity-log …`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn entry(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "occurred_at": "2026-01-01T12:00:00Z",
        "action": "created",
        "description": "Created endpoint prod-1",
        "target": { "id": "123", "kind": "endpoint", "label": "prod-1", "name": null },
        "actor": { "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "type": "user", "name": "Alice", "email": "alice@example.com" }
    })
}

#[tokio::test]
async fn activity_log_list_sends_every_filter() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs"))
        .and(query_param("q", "prod"))
        .and(query_param("actor", "7c9e6679-7425-40de-944b-e07fc1f90ae7"))
        .and(query_param("endpoint_id", "123"))
        .and(query_param("type", "endpoint"))
        .and(query_param("from", "2026-01-01T00:00:00Z"))
        .and(query_param("to", "2026-02-01T00:00:00Z"))
        .and(query_param("limit", "50"))
        .and(query_param("offset", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [entry("a1")],
            "pagination": { "total": 101, "limit": 50, "offset": 100 },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "activity-log",
            "list",
            "--query",
            "prod",
            "--actor",
            "7c9e6679-7425-40de-944b-e07fc1f90ae7",
            "--endpoint-id",
            "123",
            "--type",
            "endpoint",
            "--from",
            "2026-01-01T00:00:00Z",
            "--to",
            "2026-02-01T00:00:00Z",
            "--limit",
            "50",
            "--offset",
            "100",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn activity_log_list_without_flags_sends_no_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs"))
        .and(query_param_is_missing("q"))
        .and(query_param_is_missing("from"))
        .and(query_param_is_missing("limit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [],
            "pagination": { "total": 0, "limit": 20, "offset": 0 },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["activity", "ls"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn activity_log_list_rejects_limit_over_100() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["activity-log", "list", "--limit", "101"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn activity_log_list_403_renders_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["activity-log", "list"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("forbidden"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn activity_log_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs/filters"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "actors": [{ "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "name": "Alice", "type": "user" }],
                "types": [{ "value": "endpoint", "label": "Endpoint" }]
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["activity-log", "filters"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn activity_log_show_passes_id_in_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs/a1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": entry("a1"), "error": null })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["activity-log", "show", "a1"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn activity_log_show_404_renders_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/activity_logs/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["activity-log", "show", "missing"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("not found"), "stderr={}", out.stderr);
}
