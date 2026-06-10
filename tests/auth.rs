//! Key-resolution integration tests: `--config-file` and flag precedence.

mod common;

use common::{run_qn, run_qn_no_key};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_config(dir: &tempfile::TempDir, key: &str) -> std::path::PathBuf {
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("[api]\nkey = \"{key}\"\n")).unwrap();
    path
}

#[tokio::test]
async fn config_file_flag_supplies_the_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .and(header("x-api-key", "from-config-file"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [],
            "pagination": { "total": 0, "limit": 20, "offset": 0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, "from-config-file");
    let out = run_qn_no_key(
        &server.uri(),
        &["--config-file", cfg.to_str().unwrap(), "endpoint", "list"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn api_key_flag_wins_over_config_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints"))
        .and(header("x-api-key", "test")) // the harness-injected flag value
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [],
            "pagination": { "total": 0, "limit": 20, "offset": 0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, "from-config-file");
    let out = run_qn(
        &server.uri(),
        &["--config-file", cfg.to_str().unwrap(), "endpoint", "list"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn missing_config_file_with_no_other_source_exits_4() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("does-not-exist.toml");
    let out = run_qn_no_key(
        &server.uri(),
        &["--config-file", cfg.to_str().unwrap(), "endpoint", "list"],
    )
    .await;
    assert_eq!(out.exit_code, 4, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("no API key found"),
        "stderr={}",
        out.stderr
    );
}
