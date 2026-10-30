//! Integration tests for the 0.9.0 security controls: request-filter
//! listing and custom patterns, client certificates, the method blacklist,
//! block-range limits, and blocked IPs.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn certs_body() -> serde_json::Value {
    json!({
        "data": {
            "enabled": true,
            "certs": [{
                "fingerprint": "ab:cd:ef",
                "subject": "CN=client",
                "issuer": "CN=ca",
                "expires_at": "2027-01-01T00:00:00Z"
            }]
        },
        "error": null
    })
}

/// Writes `content` to a unique temp file and returns its path.
fn temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("qn-{}-{name}", std::process::id()));
    std::fs::write(&p, content).unwrap();
    p
}

// ---- request filters ---- //

#[tokio::test]
async fn request_filter_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/security/request_filters"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "rf-1", "pattern": { "method": ["eth_call"] }, "method_whitelist": true },
                { "id": "rf-2", "pattern": { "method": [{ "prefix": "eth_" }], "jsonrpc": ["2.0"] }, "method_whitelist": false }
            ],
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "request-filter", "list", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn request_filter_create_pattern_sends_the_json_object() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/request_filters"))
        .and(body_json(json!({
            "pattern": { "method": [{ "prefix": "eth_" }], "jsonrpc": ["2.0"] }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": "rf-2", "pattern": { "method": [{ "prefix": "eth_" }], "jsonrpc": ["2.0"] } }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "request-filter",
            "create",
            "ep-1",
            "--pattern",
            r#"{"method":[{"prefix":"eth_"}],"jsonrpc":["2.0"]}"#,
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn request_filter_create_pattern_from_file() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/request_filters"))
        .and(body_json(json!({ "pattern": { "jsonrpc": ["2.0"] } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "id": "rf-3" } })))
        .expect(1)
        .mount(&server)
        .await;
    let file = temp_file("pattern.json", r#"{"jsonrpc":["2.0"]}"#);
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "request-filter",
            "create",
            "ep-1",
            "--pattern-file",
            file.to_str().unwrap(),
        ],
    )
    .await;
    let _ = std::fs::remove_file(&file);
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn request_filter_create_rejects_pattern_with_method() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/request_filters"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "request-filter",
            "create",
            "ep-1",
            "--method",
            "eth_call",
            "--pattern",
            r#"{"jsonrpc":["2.0"]}"#,
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("not both"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn request_filter_create_rejects_non_object_pattern() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/request_filters"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "request-filter",
            "create",
            "ep-1",
            "--pattern",
            r#"["eth_call"]"#,
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("JSON object"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn request_filter_update_pattern_sends_put() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/request_filters/rf-2"))
        .and(body_json(
            json!({ "pattern": { "method": [{ "prefix": "eth_get" }] } }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "request-filter",
            "update",
            "ep-1",
            "rf-2",
            "--pattern",
            r#"{"method":[{"prefix":"eth_get"}]}"#,
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn request_filter_update_still_accepts_method_whitelist() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/request_filters/rf-1"))
        .and(body_json(
            json!({ "method": ["eth_call", "eth_blockNumber"] }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "request-filter",
            "update",
            "ep-1",
            "rf-1",
            "--methods",
            "eth_call,eth_blockNumber",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// ---- client certificates ---- //

#[tokio::test]
async fn client_cert_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(certs_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "client-cert", "list", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_list_403_renders_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "client-cert", "ls", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("forbidden"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_add_inline_sends_certificate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .and(body_json(
            json!({ "certificate": "-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(certs_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "client-cert",
            "add",
            "ep-1",
            "--certificate",
            "-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_add_from_file() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .and(body_json(json!({ "certificate": "PEM-FROM-FILE" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(certs_body()))
        .expect(1)
        .mount(&server)
        .await;
    let file = temp_file("client.pem", "PEM-FROM-FILE");
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "client-cert",
            "add",
            "ep-1",
            "--certificate-file",
            file.to_str().unwrap(),
        ],
    )
    .await;
    let _ = std::fs::remove_file(&file);
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_add_requires_a_source() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "client-cert", "add", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_enable_sends_true_without_confirmation() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .and(body_json(json!({ "enabled": true })))
        .respond_with(ResponseTemplate::new(200).set_body_json(certs_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "client-cert", "enable", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_disable_with_yes_sends_false() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .and(body_json(json!({ "enabled": false })))
        .respond_with(ResponseTemplate::new(200).set_body_json(certs_body()))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "client-cert",
            "disable",
            "ep-1",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_disable_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/security/client_certs"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "client-cert", "disable", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_remove_with_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/security/client_certs/ab:cd:ef"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": false, "certs": [] }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "client-cert",
            "remove",
            "ep-1",
            "ab:cd:ef",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn client_cert_remove_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/security/client_certs/ab:cd:ef"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "client-cert",
            "remove",
            "ep-1",
            "ab:cd:ef",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

// ---- method blacklist ---- //

#[tokio::test]
async fn method_blacklist_show() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/security/method_blacklist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "methods": ["eth_sendRawTransaction"] }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "method-blacklist", "show", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn method_blacklist_set_sends_replacement_list() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/method_blacklist"))
        .and(body_json(json!({ "methods": ["eth_sendRawTransaction", "debug_traceTransaction"] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "methods": ["eth_sendRawTransaction", "debug_traceTransaction"] }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "method-blacklist",
            "set",
            "ep-1",
            "--method",
            "eth_sendRawTransaction",
            "--methods",
            "debug_traceTransaction",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn method_blacklist_set_without_methods_fails_before_api_call() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/method_blacklist"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "method-blacklist", "set", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("clear"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn method_blacklist_clear_with_yes_sends_empty_list() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/method_blacklist"))
        .and(body_json(json!({ "methods": [] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "methods": [] }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "method-blacklist",
            "clear",
            "ep-1",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn method_blacklist_clear_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/method_blacklist"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "method-blacklist", "clear", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

// ---- range limits ---- //

#[tokio::test]
async fn range_limit_show() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/security/range_limits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "limits": { "eth_getLogs": 1000 }, "supported_methods": ["eth_getLogs", "eth_getFilterLogs"] },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "range-limit", "show", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn range_limit_set_sends_method_map() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/range_limits"))
        .and(body_json(json!({ "limits": { "eth_getLogs": 1000, "eth_getFilterLogs": 500 } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "limits": { "eth_getLogs": 1000, "eth_getFilterLogs": 500 }, "supported_methods": ["eth_getLogs", "eth_getFilterLogs"] },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "range-limit",
            "set",
            "ep-1",
            "--limit",
            "eth_getLogs=1000",
            "--limit",
            "eth_getFilterLogs=500",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn range_limit_set_rejects_a_bad_pair_before_api_call() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/range_limits"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "range-limit",
            "set",
            "ep-1",
            "--limit",
            "eth_getLogs=0",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("positive"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn range_limit_clear_with_yes_sends_empty_object() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/range_limits"))
        .and(body_json(json!({ "limits": {} })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "limits": {}, "supported_methods": ["eth_getLogs"] }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "range-limit",
            "clear",
            "ep-1",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn range_limit_clear_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v0/endpoints/ep-1/security/range_limits"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "range-limit", "clear", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

// ---- blocked IPs ---- //

#[tokio::test]
async fn blocked_ip_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/security/blocked_ips"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "bip-1", "ip": "192.0.2.0/24" }], "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "blocked-ip", "list", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn blocked_ip_add_sends_ip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/blocked_ips"))
        .and(body_json(json!({ "ip": "192.0.2.0/24" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": "bip-1", "ip": "192.0.2.0/24" }, "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "blocked-ip",
            "add",
            "ep-1",
            "192.0.2.0/24",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn blocked_ip_remove_with_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/security/blocked_ips/bip-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": true })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "blocked-ip",
            "remove",
            "ep-1",
            "bip-1",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn blocked_ip_remove_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/security/blocked_ips/bip-1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "blocked-ip",
            "remove",
            "ep-1",
            "bip-1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn blocked_ip_list_404_renders_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-9/security/blocked_ips"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "blocked-ip", "list", "ep-9"],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("not found"), "stderr={}", out.stderr);
}
