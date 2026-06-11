//! Integration tests for `qn endpoint …` against wiremock.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn endpoint_payload(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": "test-name",
        "label": "test-label",
        "status": "active",
        "chain": "ethereum",
        "network": "mainnet",
        "is_dedicated": false,
        "is_flat_rate": false,
        "http_url": format!("https://{id}.example/v1/abc"),
        "wss_url": null,
        "tags": [],
        "is_multichain": false,
    })
}

/// Subprocess test: confirms `--format json` output is valid JSON over the wire.
#[tokio::test]
async fn list_endpoints_json_output_is_valid_json() {
    use assert_cmd::Command;
    use std::process::Stdio;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [endpoint_payload("ep-1")],
            "pagination": { "total": 1, "limit": 20, "offset": 0 }
        })))
        .mount(&server)
        .await;
    let _ = Stdio::null();

    let output = Command::cargo_bin("qn")
        .unwrap()
        .env_remove("HOME")
        .env("HOME", std::env::temp_dir())
        .args([
            "--api-key",
            "test",
            "--base-url",
            &server.uri(),
            "--no-input",
            "--format",
            "json",
            "endpoint",
            "list",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nstdout was:\n{stdout}"));
    assert_eq!(v["data"][0]["id"].as_str(), Some("ep-1"));
}

/// Subprocess test: each non-table format produces *some* stdout containing the
/// endpoint ID. The exact byte content is format-specific; we don't want to
/// over-fit on serializer details. The point is: the format flag is wired
/// through, exit 0, and the ID shows up somewhere.
#[tokio::test]
async fn list_endpoints_all_formats_render() {
    use assert_cmd::Command;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [endpoint_payload("ep-1")],
            "pagination": { "total": 1, "limit": 20, "offset": 0 }
        })))
        .mount(&server)
        .await;

    for fmt in ["json", "yaml", "md", "toon", "table"] {
        let output = Command::cargo_bin("qn")
            .unwrap()
            .env_remove("HOME")
            .env("HOME", std::env::temp_dir())
            .args([
                "--api-key",
                "test",
                "--base-url",
                &server.uri(),
                "--no-input",
                "--no-color",
                "-o",
                fmt,
                "endpoint",
                "list",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "format={fmt} exit={:?} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("ep-1"),
            "format={fmt} stdout did not contain ep-1:\n{stdout}"
        );
    }
}

/// `--wide -o md` adds HTTP/WSS columns to the markdown table.
#[tokio::test]
async fn list_endpoints_wide_md_includes_urls() {
    use assert_cmd::Command;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [endpoint_payload("ep-1")],
            "pagination": { "total": 1, "limit": 20, "offset": 0 }
        })))
        .mount(&server)
        .await;

    let output = Command::cargo_bin("qn")
        .unwrap()
        .env_remove("HOME")
        .env("HOME", std::env::temp_dir())
        .args([
            "--api-key",
            "test",
            "--base-url",
            &server.uri(),
            "--no-input",
            "--no-color",
            "-o",
            "md",
            "-w",
            "endpoint",
            "list",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("HTTP") && stdout.contains("WSS"),
        "expected HTTP/WSS headers in -w output:\n{stdout}"
    );
    assert!(
        stdout.contains("ep-1.example"),
        "expected http_url in -w output:\n{stdout}"
    );
}

#[tokio::test]
async fn list_endpoints_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .and(header("x-api-key", "test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [endpoint_payload("ep-1"), endpoint_payload("ep-2")],
            "pagination": { "total": 2, "limit": 20, "offset": 0 }
        })))
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["endpoint", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn requests_carry_the_cli_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .and(header("user-agent", qn::context::user_agent().as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [],
            "pagination": { "total": 0, "limit": 20, "offset": 0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["endpoint", "list"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn list_endpoints_404_renders_clean_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{\"message\":\"nope\"}"))
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), &["endpoint", "list"]).await;
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.contains("not found"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn list_endpoints_401_maps_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .respond_with(ResponseTemplate::new(401).set_body_string("denied"))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["endpoint", "list"]).await;
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.contains("unauthorized"), "stderr={}", out.stderr);
}

#[tokio::test]
async fn create_endpoint_sends_chain_and_network() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints"))
        .and(body_json(
            json!({ "chain": "ethereum", "network": "mainnet" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": "ep-new",
                "chain": "ethereum",
                "network": "mainnet",
                "http_url": "https://ep-new.example",
                "tags": []
            }
        })))
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
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn show_endpoint_passes_id_in_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": "ep-show",
                "chain": "solana",
                "network": "mainnet",
                "http_url": "https://ep-show.example",
                "tags": []
            }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["endpoint", "show", "ep-show"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn update_endpoint_label() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1"))
        .and(body_json(json!({ "label": "new label" })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "update", "ep-1", "--label", "new label"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn archive_endpoint_requires_yes_in_no_tty() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let no_yes = run_qn(&server.uri(), &["endpoint", "archive", "ep-1"]).await;
    assert_eq!(no_yes.exit_code, 5);
    assert!(no_yes.stderr.contains("confirmation"), "{}", no_yes.stderr);

    let with_yes = run_qn(&server.uri(), &["endpoint", "archive", "ep-1", "--yes"]).await;
    assert_eq!(with_yes.exit_code, 0, "stderr={}", with_yes.stderr);
}

#[tokio::test]
async fn pause_resume_send_status_update() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/status"))
        .and(body_json(json!({ "status": "paused" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":"ok"})))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/status"))
        .and(body_json(json!({ "status": "active" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":"ok"})))
        .mount(&server)
        .await;
    assert_eq!(
        run_qn(&server.uri(), &["endpoint", "pause", "ep-1"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["endpoint", "resume", "ep-1"])
            .await
            .exit_code,
        0
    );
}

#[tokio::test]
async fn urls_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/ep-1/urls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "http_url": "https://ep-1.example",
                "wss_url": "wss://ep-1.example",
                "multichain_urls": null
            }
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["endpoint", "urls", "ep-1"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn multichain_enable_disable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/enable_multichain"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/disable_multichain"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert_eq!(
        run_qn(&server.uri(), &["endpoint", "enable-multichain", "ep-1"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["endpoint", "disable-multichain", "ep-1"])
            .await
            .exit_code,
        0
    );
}

#[tokio::test]
async fn endpoint_tag_add() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/tags"))
        .and(body_json(json!({ "label": "prod" })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["endpoint", "tag", "add", "ep-1", "prod"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_security_token_create() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/security/tokens"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "token", "create", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_security_token_delete_with_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/security/tokens/tok-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": true })))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint", "security", "token", "delete", "ep-1", "tok-1", "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_security_token_delete_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/security/tokens/tok-1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "token", "delete", "ep-1", "tok-1"],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn rate_limit_delete_override_with_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/rate-limits/ovr-1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "rate-limit",
            "delete-override",
            "ep-1",
            "ovr-1",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn rate_limit_delete_override_without_yes_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v0/endpoints/ep-1/rate-limits/ovr-1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "rate-limit", "delete-override", "ep-1", "ovr-1"],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_security_set_options_sends_partial_payload() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/security_options"))
        .and(body_json(json!({
            "options": { "tokens": "enabled", "jwts": "disabled" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "security",
            "set-options",
            "ep-1",
            "--tokens",
            "enabled",
            "--jwts",
            "disabled",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_method_ratelimit_create() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/method-rate-limits"))
        .and(body_json(json!({
            "interval": "second",
            "methods": ["eth_call"],
            "rate": 10
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": "rl-1",
                "interval": "second",
                "methods": ["eth_call"],
                "rate": 10,
                "status": "enabled",
                "created": "2026-01-01T00:00:00Z"
            }
        })))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "endpoint",
            "rate-limit",
            "method-create",
            "ep-1",
            "--interval",
            "second",
            "--method",
            "eth_call",
            "--rate",
            "10",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn endpoint_ratelimit_set_omits_unset_fields() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/v0/endpoints/ep-1/rate-limits"))
        .and(body_json(json!({ "rate_limits": { "rps": 100 } })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "rate-limit", "set", "ep-1", "--rps", "100"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// ---- Required-args enforcement (clap-level, exits 1 before any HTTP) ---- //

#[tokio::test]
async fn endpoint_create_no_flags_fails_before_api_call() {
    // No mocks mounted; if the CLI tries to make a request, wiremock would 404.
    // clap rejects the invocation *before* any HTTP call.
    let server = MockServer::start().await;
    let out = run_qn(&server.uri(), &["endpoint", "create"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("--chain") && out.stderr.contains("--network"),
        "stderr={}",
        out.stderr
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn endpoint_create_only_chain_fails_before_api_call() {
    let server = MockServer::start().await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "create", "--chain", "ethereum"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("--network <NETWORK>"),
        "should call out --network; stderr={}",
        out.stderr
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn endpoint_update_no_flags_fails_before_api_call() {
    let server = MockServer::start().await;
    let out = run_qn(&server.uri(), &["endpoint", "update", "ep-1"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("--label"), "stderr={}", out.stderr);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn stream_create_missing_flags_fails_before_api_call() {
    let server = MockServer::start().await;
    let out = run_qn(&server.uri(), &["stream", "create", "--name", "x"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("--network") && out.stderr.contains("--webhook"),
        "stderr={}",
        out.stderr
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn endpoint_security_set_options_no_flags_fails_before_api_call() {
    let server = MockServer::start().await;
    let out = run_qn(
        &server.uri(),
        &["endpoint", "security", "set-options", "ep-1"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(out.stderr.contains("--tokens"), "stderr={}", out.stderr);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}
