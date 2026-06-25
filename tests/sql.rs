//! Integration tests for `qn sql …`.

mod common;

use common::run_qn;
use serde_json::json;
use std::io::Write;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn query_body() -> serde_json::Value {
    json!({
        "meta": [
            {"name": "time", "type": "DateTime('UTC')"},
            {"name": "action_type", "type": "LowCardinality(String)"}
        ],
        "data": [
            {"time": "2026-06-24 19:43:44", "action_type": "SystemSpotSendAction"},
            {"time": "2026-06-24 19:43:42", "action_type": "SystemSendAssetAction"}
        ],
        "rows": 2,
        "rows_before_limit_at_least": 18251,
        "statistics": {"elapsed": 0.0067, "rows_read": 31341, "bytes_read": 1247178},
        "credits": 135
    })
}

#[tokio::test]
async fn query_inline_sends_camel_case_cluster_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .and(body_partial_json(json!({
            "query": "SELECT 1",
            "clusterId": "hyperliquid-core-mainnet"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "sql",
            "query",
            "SELECT 1",
            "--cluster-id",
            "hyperliquid-core-mainnet",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn query_reads_from_file() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .and(body_partial_json(json!({ "query": "SELECT 42 FROM t" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("query.sql");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "SELECT 42 FROM t").unwrap();

    let out = run_qn(
        &server.uri(),
        &[
            "sql",
            "query",
            "--file",
            path.to_str().unwrap(),
            "--cluster-id",
            "c1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn query_missing_file_is_arg_error() {
    let server = MockServer::start().await;
    // No request should reach the server when the file can't be read.
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "sql",
            "query",
            "--file",
            "/no/such/query.sql",
            "--cluster-id",
            "c1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn query_requires_a_source() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .expect(0)
        .mount(&server)
        .await;
    // Neither inline SQL nor --file: clap ArgGroup rejects (parse error, exit 1).
    let out = run_qn(&server.uri(), &["sql", "query", "--cluster-id", "c1"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn query_inline_and_file_conflict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .expect(0)
        .mount(&server)
        .await;
    // Both inline SQL and --file: clap ArgGroup rejects (parse error, exit 1).
    let out = run_qn(
        &server.uri(),
        &[
            "sql",
            "query",
            "SELECT 1",
            "--file",
            "q.sql",
            "--cluster-id",
            "c1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn query_api_error_maps_to_exit_2() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            json!({"statusCode": 403, "message": "only SELECT queries are allowed"}),
        ))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["sql", "query", "DELETE FROM t", "--cluster-id", "c1"],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}

#[tokio::test]
async fn schema_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sql/rest/v1/schema/hyperliquid-core-mainnet"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "chain": "Hyperliquid (HyperCore)",
            "cluster_id": "hyperliquid-core-mainnet",
            "tables": [
                {
                    "name": "hyperliquid_agents",
                    "engine": "SharedReplacingMergeTree",
                    "total_rows": 3322574607i64,
                    "partition_key": "toYYYYMM(snapshot_time)",
                    "sorting_key": ["block_number", "agent"],
                    "columns": [
                        {"name": "agent", "type": "FixedString(42)"},
                        {"name": "block_number", "type": "UInt64"}
                    ]
                }
            ]
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["sql", "schema", "hyperliquid-core-mainnet"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn schema_not_found_maps_to_exit_2() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sql/rest/v1/schema/bad-cluster"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["sql", "schema", "bad-cluster"]).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}
