//! Snapshot tests for table output through the real binary.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount a GET response and return table stdout.
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
                    "hsts": true,
                    "cors": false,
                    "responseLogging": true,
                    "ipCustomHeader": { "value": "x-real-ip" }
                },
                "tokens": [
                    { "id": "tok-1", "token": "0xabc" }
                ],
                "jwts": null,
                "referrers": null,
                "domain_masks": null,
                "ips": null,
                "blocked_ips": [
                    { "id": "bip-1", "ip": "198.51.100.0/24" }
                ],
                "request_filters": [
                    { "id": "rf-1", "method": ["eth_blockNumber", "eth_call"] },
                    { "id": "rf-2", "method": [{ "prefix": "eth_" }], "params": { "jsonrpc": ["2.0"] } }
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
                "hsts": true,
                "cors": true,
                "responseLogging": false,
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
            "blocked_ips": [
                { "id": "bip-1", "ip": "198.51.100.0/24" }
            ],
            "request_filters": [
                { "id": "rf-1", "method": ["eth_blockNumber", "eth_call"] },
                { "id": "rf-2", "method": [{ "prefix": "eth_" }], "params": { "jsonrpc": ["2.0"], "id": [1] } }
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
async fn endpoint_method_rate_limit_list_shows_limit_by_ip() {
    let body = serde_json::json!({
        "data": {
            "rate_limiters": [
                { "id": "rl-1", "interval": "second", "methods": ["eth_call"], "rate": 10,
                  "status": "enabled", "created": "2026-01-01T00:00:00Z", "limit_by_ip": true },
                { "id": "rl-2", "interval": "minute", "methods": ["eth_getLogs"], "rate": 5,
                  "status": "disabled", "created": "2026-01-01T00:00:00Z", "limit_by_ip": null }
            ]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/method-rate-limits",
        body,
        &["endpoint", "rate-limit", "method-list", "ep-1"],
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
async fn usage_by_endpoint_wide_table_includes_credit_breakdown() {
    let body = serde_json::json!({
        "data": {
            "endpoints": [
                { "name": "ep-1", "chain": "eth", "network": "mainnet", "credits_used": 1200,
                  "methods_breakdown": [], "requests": 40, "rpc_credits": 1000, "data_credits": 200, "bytes": 4096 },
                { "name": "ep-2", "chain": "solana", "network": "mainnet", "credits_used": 300,
                  "methods_breakdown": [], "requests": null, "rpc_credits": null, "data_credits": null, "bytes": null }
            ]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/usage/rpc/by-endpoint",
        body,
        &["usage", "by-endpoint", "--wide"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn usage_rpc_summary_table_groups_by_selected_dimensions() {
    let body = serde_json::json!({
        "data": {
            "from": "2026-01-01T00:00:00Z",
            "to": "2026-01-02T00:00:00Z",
            "totals": { "requests": 10, "bytes": 2048, "rpc_credits": 8, "data_credits": 2.5, "credits": 10.5 },
            "breakdown": [
                { "method": "eth_call", "endpoint": 123, "requests": 7, "bytes": 1024, "rpc_credits": 6, "data_credits": 1, "credits": 7 },
                { "method": null, "endpoint": 456, "requests": null, "bytes": 1024, "rpc_credits": 2, "data_credits": 1.5, "credits": 3.5 }
            ],
            "next_cursor": "eyJvZmZzZXQiOjEwMH0"
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/usage/rpc/summary",
        body,
        &[
            "usage",
            "rpc",
            "--from",
            "24h",
            "--group-by",
            "method,endpoint",
        ],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn usage_credits_summary_table_without_breakdown() {
    let body = serde_json::json!({
        "data": {
            "from": "2026-01-01T00:00:00Z",
            "to": "2026-01-02T00:00:00Z",
            "totals": { "credits": 1200 }
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/usage/credits/summary",
        body,
        &["usage", "credits", "--from", "24h"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn metrics_errors_summary_table_shows_kind_and_count() {
    let body = serde_json::json!({
        "data": {
            "from": "2026-01-01T00:00:00Z",
            "to": "2026-01-02T00:00:00Z",
            "totals": { "rpc_errors": 5, "http_errors": 2 },
            "breakdown": [
                { "kind": "rpc", "count": 4, "code": "-32000", "method": "eth_call" },
                { "kind": "rpc", "count": 1, "code": "-32602", "method": null }
            ]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/metrics/errors/summary",
        body,
        &[
            "metrics",
            "errors",
            "--from",
            "24h",
            "--kind",
            "rpc",
            "--group-by",
            "code,method",
        ],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn activity_log_list_table_shows_target_actor_and_pagination() {
    let body = serde_json::json!({
        "data": [
            {
                "id": "a1", "occurred_at": "2026-01-01T12:00:00Z", "action": "created",
                "description": "Created endpoint prod-1",
                "target": { "id": "123", "kind": "endpoint", "label": "prod-1", "name": null },
                "actor": { "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "type": "user", "name": "Alice", "email": "alice@example.com" }
            },
            {
                "id": "a2", "occurred_at": "2026-01-01T11:00:00Z", "action": "deleted",
                "description": null,
                "target": { "id": "t-9", "kind": null, "label": "old-team", "name": null },
                "actor": { "id": null, "type": "deleted_user", "name": null, "email": null }
            }
        ],
        "pagination": { "total": 42, "limit": 2, "offset": 0 },
        "error": null
    });
    let out = table_stdout(
        "/v0/activity_logs",
        body,
        &["activity-log", "list", "--limit", "2"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn activity_log_filters_table_lists_actors_and_types() {
    let body = serde_json::json!({
        "data": {
            "actors": [
                { "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "name": "Alice", "type": "user" },
                { "id": "0f8fad5b-d9cb-469f-a165-70867728950e", "name": null, "type": "api_key" }
            ],
            "types": [
                { "value": "endpoint", "label": "Endpoint" },
                { "value": "team", "label": "Team" }
            ]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/activity_logs/filters",
        body,
        &["activity-log", "filters"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn transaction_list_table_shows_endpoints_and_cursor() {
    let body = serde_json::json!({
        "data": {
            "from": "2026-01-01T00:00:00Z",
            "to": "2026-01-02T00:00:00Z",
            "transactions": [{
                "network": "ethereum-mainnet", "transaction_hash": "0xabc",
                "first_submitted_at": "2026-01-01T10:00:00Z", "last_submitted_at": "2026-01-01T10:05:00Z",
                "attempt_count": 2, "endpoint_ids": [123, null], "methods": ["eth_sendRawTransaction"],
                "archive": false, "height": "9007199254740993", "state": "succeeded"
            }],
            "next_cursor": "eyJvZmZzZXQiOjEwMH0"
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/transactions",
        body,
        &["transaction", "list", "--from", "24h"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn transaction_show_table_with_payloads() {
    let body = serde_json::json!({
        "data": {
            "exists": true,
            "data": {
                "network": "ethereum-mainnet", "transaction_hash": "0xabc",
                "first_submitted_at": "2026-01-01T10:00:00Z", "last_submitted_at": "2026-01-01T10:05:00Z",
                "state": "succeeded", "attempt_count": 2, "endpoint_ids": [123, null],
                "methods": ["eth_sendRawTransaction"], "archive": false, "height": "9007199254740993",
                "attempts": [
                    { "submitted_at": "2026-01-01T10:00:00Z", "endpoint_id": 123, "method": "eth_sendRawTransaction", "request_id": "req-1",
                      "payloads": { "request": "{\"method\":\"eth_sendRawTransaction\"}", "response": "not json" } },
                    { "submitted_at": "2026-01-01T10:05:00Z", "endpoint_id": 123, "method": "eth_sendRawTransaction", "request_id": "req-2",
                      "payloads": { "request": null, "response": null } }
                ]
            }
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/transactions/ethereum-mainnet/0xabc",
        body,
        &[
            "transaction",
            "show",
            "ethereum-mainnet",
            "0xabc",
            "--payloads",
        ],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn transaction_show_table_when_not_retained() {
    let body = serde_json::json!({ "data": { "exists": false, "data": null }, "error": null });
    let out = table_stdout(
        "/v0/transactions/ethereum-mainnet/0xdead",
        body,
        &["transaction", "show", "ethereum-mainnet", "0xdead"],
    )
    .await;
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

/// Mount a POST response and return table stdout.
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
async fn endpoint_security_request_filter_list_table() {
    let body = serde_json::json!({
        "data": [
            { "id": "rf-1", "pattern": { "method": ["eth_call", "eth_blockNumber"] }, "method_whitelist": true },
            { "id": "rf-2", "pattern": { "method": [{ "prefix": "eth_" }], "jsonrpc": ["2.0"] }, "method_whitelist": false }
        ],
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/security/request_filters",
        body,
        &["endpoint", "security", "request-filter", "list", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_security_client_cert_list_table() {
    let body = serde_json::json!({
        "data": {
            "enabled": true,
            "certs": [
                { "fingerprint": "ab:cd:ef", "subject": "CN=client", "issuer": "CN=ca", "expires_at": "2027-01-01T00:00:00Z" }
            ]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/security/client_certs",
        body,
        &["endpoint", "security", "client-cert", "list", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_security_range_limit_show_table() {
    let body = serde_json::json!({
        "data": {
            "limits": { "eth_getLogs": 1000, "eth_getFilterLogs": 500 },
            "supported_methods": ["eth_getLogs", "eth_getFilterLogs", "eth_newFilter"]
        },
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/security/range_limits",
        body,
        &["endpoint", "security", "range-limit", "show", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_security_blocked_ip_list_table() {
    let body = serde_json::json!({
        "data": [
            { "id": "bip-1", "ip": "192.0.2.0/24" },
            { "id": "bip-2", "ip": "198.51.100.7" }
        ],
        "error": null
    });
    let out = table_stdout(
        "/v0/endpoints/ep-1/security/blocked_ips",
        body,
        &["endpoint", "security", "blocked-ip", "list", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_security_token_create_prints_the_token() {
    let body = serde_json::json!({ "data": { "id": "tok-1", "token": "0xabc" }, "error": null });
    let out = table_stdout_post(
        "/v0/endpoints/ep-1/security/tokens",
        body,
        &["endpoint", "security", "token", "create", "ep-1"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn endpoint_tag_add_prints_the_association() {
    let body = serde_json::json!({
        "data": { "tags": [
            { "id": 10, "tag_id": 1, "account_id": 1, "endpoint_id": 5, "label": "prod",
              "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z" }
        ] },
        "error": null
    });
    let out = table_stdout_post(
        "/v0/endpoints/ep-1/tags",
        body,
        &["endpoint", "tag", "add", "ep-1", "prod"],
    )
    .await;
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn bulk_pause_table_shows_per_item_error() {
    let body = serde_json::json!({
        "data": {
            "total": 2, "updated_count": 1, "failed_count": 1,
            "results": [
                { "id": "ep-1", "success": true },
                { "id": "ep-2", "success": false, "error": "endpoint not found" }
            ]
        },
        "error": null
    });
    let out = table_stdout_post(
        "/v0/endpoints/bulk/status",
        body,
        &["endpoint", "bulk", "pause", "ep-1", "ep-2", "--yes"],
    )
    .await;
    insta::assert_snapshot!(out);
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

/// Run an RPC discovery command and return table stdout.
async fn discovery_stdout(server: &MockServer, scheme: &str, verb: &str) -> String {
    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .env_remove("HOME")
        .env("HOME", std::env::temp_dir())
        .args([
            "--base-url",
            &server.uri(),
            "--no-input",
            "--no-color",
            "--format",
            "table",
            "rpc",
            scheme,
            verb,
        ])
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
async fn x402_supported_networks_table_lists_slugs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "networks": ["base-sepolia", "ethereum-mainnet", "solana-devnet"]
        })))
        .mount(&server)
        .await;

    insta::assert_snapshot!(discovery_stdout(&server, "x402", "supported-networks").await);
}

#[tokio::test]
async fn x402_supported_payments_table_lists_options() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/supported"))
        .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
            "x402Version": 2,
            "accepts": [{
                "scheme": "exact",
                "network": "eip155:84532",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "extra": {"name": "USDC", "version": "2"}
            }]
        })))
        .mount(&server)
        .await;

    insta::assert_snapshot!(discovery_stdout(&server, "x402", "supported-payments").await);
}

#[tokio::test]
async fn mpp_supported_payments_table_lists_options() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "networks": ["base-sepolia", "tempo-testnet"]
        })))
        .mount(&server)
        .await;
    let tempo = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "amount": "1000",
            "currency": "0x20c0000000000000000000000000000000000000",
            "methodDetails": {"chainId": 42431},
            "recipient": "0x0000000000000000000000000000000000000002"
        })
        .to_string(),
    );
    let header = format!(
        "Payment id=\"a\", realm=\"mock\", method=\"tempo\", intent=\"charge\", \
         request=\"{tempo}\", description=\"Quicknode RPC request\""
    );
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(402).insert_header("www-authenticate", header.as_str()))
        .mount(&server)
        .await;

    insta::assert_snapshot!(discovery_stdout(&server, "mpp", "supported-payments").await);
}
