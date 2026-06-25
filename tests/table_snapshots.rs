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
                    "requestFilters": true,
                    "ipCustomHeader": { "value": "x-real-ip" }
                },
                "tokens": [
                    { "id": "tok-1", "token": "0xabc" }
                ],
                "jwts": null,
                "referrers": null,
                "domain_masks": null,
                "ips": null,
                "request_filters": [
                    { "id": "rf-1", "method": ["eth_blockNumber", "eth_call"] }
                ]
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
async fn endpoint_logs_table_includes_error_code() {
    let body = serde_json::json!({
        "data": [
            {
                "timestamp": "2026-01-01T00:00:00.000Z",
                "method": "eth_blockNumbre",
                "network": "mainnet",
                "http_method": "POST",
                "status": 200,
                "error_code": -32601,
                "url": "/",
                "request_id": "req-1",
                "details": null
            },
            {
                "timestamp": "2026-01-01T00:00:01.000Z",
                "method": "eth_blockNumber",
                "network": "mainnet",
                "http_method": "POST",
                "status": 200,
                "error_code": null,
                "url": "/",
                "request_id": "req-2",
                "details": null
            }
        ],
        "next_at": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/logs",
        body,
        &["endpoint", "logs", "ep-1", "--from", "2026-01-01T00:00:00Z"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn team_show_lists_members_and_pending_invites() {
    let body = serde_json::json!({
        "data": {
            "id": 7,
            "name": "core",
            "default_role": "viewer",
            "members_count": 2,
            "users": [
                {
                    "id": 1,
                    "full_name": "Alice Example",
                    "email": "alice@example.com",
                    "role": "admin",
                    "status": "active",
                    "created_at": "2026-01-01T00:00:00Z",
                    "photo_url": null
                },
                {
                    "id": 2,
                    "full_name": null,
                    "email": "bob@example.com",
                    "role": "viewer",
                    "status": "active",
                    "created_at": null,
                    "photo_url": null
                }
            ],
            "pending_invites": [
                {
                    "id": 3,
                    "full_name": null,
                    "email": "carol@example.com",
                    "role": "viewer",
                    "status": "pending",
                    "created_at": null,
                    "photo_url": null
                }
            ]
        },
        "error": null
    });
    let out = table_stdout("/v0/teams/7", body, &["team", "show", "7"]).await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn team_show_without_members_renders_fields_only() {
    let body = serde_json::json!({
        "data": { "id": 7, "name": "core", "default_role": null }
    });
    let out = table_stdout("/v0/teams/7", body, &["team", "show", "7"]).await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn billing_payments_table_includes_status_and_marketplace() {
    let body = serde_json::json!({
        "data": {
            "payments": [
                {
                    "amount": "49.00",
                    "card_last_4": "4242",
                    "created_at": "2026-01-01T00:00:00Z",
                    "currency": "usd",
                    "status": "succeeded",
                    "marketplace_amount": "9.00"
                },
                {
                    "amount": "49.00",
                    "card_last_4": null,
                    "created_at": "2026-02-01T00:00:00Z",
                    "currency": "usd",
                    "status": "failed",
                    "marketplace_amount": null
                }
            ]
        },
        "error": null
    });
    let out = table_stdout("/v0/billing/payments", body, &["billing", "payments"]).await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn usage_summary_table_shows_overages_and_window() {
    let body = serde_json::json!({
        "data": {
            "credits_used": 1200,
            "credits_remaining": 8800,
            "limit": 10000,
            "overages": 0,
            "start_time": 1767225600,
            "end_time": 1769904000
        },
        "error": null
    });
    let out = table_stdout("/v0/usage/rpc", body, &["usage", "summary"]).await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn usage_by_method_table_includes_chain_and_network() {
    let body = serde_json::json!({
        "data": {
            "methods": [
                {
                    "method_name": "eth_call",
                    "credits_used": 900,
                    "archive": false,
                    "network": "mainnet",
                    "chain": "eth"
                },
                {
                    "method_name": "getBlockHeight",
                    "credits_used": 300,
                    "archive": null,
                    "network": "mainnet",
                    "chain": "solana"
                }
            ]
        },
        "error": null
    });
    let out = table_stdout("/v0/usage/rpc/by-method", body, &["usage", "by-method"]).await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn usage_by_tag_table_includes_requests() {
    let body = serde_json::json!({
        "data": {
            "tags": [
                { "tag_id": 1, "label": "prod", "credits_used": 1000, "requests": 420 },
                { "tag_id": null, "label": "untagged", "credits_used": 200, "requests": 80 }
            ]
        },
        "error": null
    });
    let out = table_stdout("/v0/usage/rpc/by-tag", body, &["usage", "by-tag"]).await;
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

/// Like [`table_stdout`] but mounts the body at `POST url_path`, for commands
/// that issue a POST (e.g. `sql query`).
async fn table_stdout_post(url_path: &str, body: serde_json::Value, args: &[&str]) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
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
async fn sql_query_table_renders_unquoted_scalars_and_null_dash() {
    let body = serde_json::json!({
        "meta": [
            {"name": "action_type", "type": "LowCardinality(String)"},
            {"name": "block", "type": "UInt64"},
            {"name": "user", "type": "Nullable(String)"}
        ],
        "data": [
            {"action_type": "SystemSpotSendAction", "block": 1234567, "user": "0xabc"},
            {"action_type": "SystemSendAssetAction", "block": 1234566, "user": null}
        ],
        "rows": 2,
        "rows_before_limit_at_least": 2,
        "statistics": {"elapsed": 0.0067, "rows_read": 31341, "bytes_read": 1247178},
        "credits": 135
    });
    let out = table_stdout_post(
        "/sql/rest/v1/query",
        body,
        &["sql", "query", "SELECT 1", "--cluster-id", "c1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn sql_schema_table_renders_nested_table_blocks() {
    let body = serde_json::json!({
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
            },
            {
                "name": "hyperliquid_agents_view",
                "engine": "View",
                "total_rows": 0,
                "partition_key": "",
                "sorting_key": [],
                "columns": [
                    {"name": "agent", "type": "FixedString(42)"}
                ]
            }
        ]
    });
    let out = table_stdout(
        "/sql/rest/v1/schema/hyperliquid-core-mainnet",
        body,
        &["sql", "schema", "hyperliquid-core-mainnet"],
    )
    .await;
    insta::assert_snapshot!(out);
}
