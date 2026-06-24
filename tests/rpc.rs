//! Integration tests for `qn rpc`.
//!
//! The in-process harness injects `--api-key test`; the token cache is keyed by
//! the SHA-256 of that key. Tests that exercise the cache pass `--config-file`
//! so `tokens.toml` lands in a tempdir rather than the real home.

mod common;

use common::run_qn;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// SHA-256 of "test" (the harness-injected API key).
const TEST_KEY_HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

// A far-future ISO timestamp so a freshly minted token is never near expiry.
const FUTURE_ISO: &str = "2099-01-01T00:00:00.000Z";
// Matching far-future unix seconds for a seeded token.
const FUTURE_UNIX: i64 = 4_070_908_800;

fn write_token_cache(dir: &tempfile::TempDir, endpoint_url: &str, key_hash: &str) {
    let path = dir.path().join("tokens.toml");
    let body = format!(
        "[token]\nkey_hash = \"{key_hash}\"\nendpoint_url = \"{endpoint_url}\"\n\
         token = \"seeded.jwt\"\nexp_unix = {FUTURE_UNIX}\n"
    );
    std::fs::write(&path, body).unwrap();
}

fn cfg_path(dir: &tempfile::TempDir) -> String {
    // Provide an API key via the config file so the cache parent dir is the
    // tempdir. The harness still injects --api-key test, which wins, so the
    // cache key_hash matches TEST_KEY_HASH.
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[api]\nkey = \"test\"\n").unwrap();
    path.to_str().unwrap().to_string()
}

#[tokio::test]
async fn seeded_token_skips_mint() {
    let server = MockServer::start().await;
    // RPC endpoint returns a result. The mint route is intentionally NOT
    // mounted: if the SDK tried to mint, the call would 404 and fail.
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .and(body_partial_json(json!({ "method": "eth_blockNumber" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0x1335f9a"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, &format!("{}/rpc", server.uri()), TEST_KEY_HASH);

    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// An already-enabled call must NOT incur the post-enable provisioning wait,
// even with --yes. --yes only matters if Tooling Access is disabled; here the
// seeded token works on the first attempt, so the ~1s wait must not fire.
#[tokio::test]
async fn already_enabled_yes_does_not_wait() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0x1"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, &format!("{}/rpc", server.uri()), TEST_KEY_HASH);

    let started = std::time::Instant::now();
    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber", "--yes"],
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    // The post-enable initial wait is 1s; a happy-path call must be far faster.
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "already-enabled --yes should not wait, took {elapsed:?}"
    );
}

#[tokio::test]
async fn no_cache_mints_then_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/tooling-access/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "endpoint_url": format!("{}/rpc", server.uri()),
                "token": "minted.jwt",
                "expires_at": FUTURE_ISO
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0xabc"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    // The minted token should have been written back to the cache.
    let cached = std::fs::read_to_string(dir.path().join("tokens.toml")).unwrap();
    assert!(cached.contains("minted.jwt"), "cache: {cached}");
}

#[tokio::test]
async fn json_rpc_error_exits_nonzero() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32602, "message": "invalid params" }
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, &format!("{}/rpc", server.uri()), TEST_KEY_HASH);

    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_getBalance", "[\"bad\"]"],
    )
    .await;
    // SdkError::Rpc is neither Api nor Http → generic exit 1.
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

#[tokio::test]
async fn not_enabled_without_yes_is_actionable_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/tooling-access/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "data": null,
            "error": "Tooling access is not enabled. Enable it first."
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    // No --yes, and the harness sets --no-input, so we can't prompt → the
    // command should fail with the actionable "run 'qn tooling-access enable'".
    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("tooling-access enable"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn not_enabled_with_yes_auto_enables_and_retries() {
    let server = MockServer::start().await;

    // Mint: first call 400 (not enabled), subsequent calls succeed.
    struct MintSeq {
        calls: AtomicUsize,
        url: String,
    }
    impl Respond for MintSeq {
        fn respond(&self, _: &Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ResponseTemplate::new(400).set_body_json(json!({
                    "data": null,
                    "error": "Tooling access is not enabled. Enable it first."
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({
                    "data": { "endpoint_url": self.url, "token": "minted.jwt", "expires_at": FUTURE_ISO },
                    "error": null
                }))
            }
        }
    }

    Mock::given(method("POST"))
        .and(path("/v0/tooling-access/token"))
        .respond_with(MintSeq {
            calls: AtomicUsize::new(0),
            url: format!("{}/rpc", server.uri()),
        })
        .mount(&server)
        .await;
    // enable_tooling_access PATCH.
    Mock::given(method("PATCH"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": true, "endpoint_url": format!("{}/rpc", server.uri()) },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0xok"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber", "--yes"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn account_switch_invalidates_cached_token() {
    let server = MockServer::start().await;
    // A cache entry scoped to a DIFFERENT key. The SDK must ignore it and mint.
    Mock::given(method("POST"))
        .and(path("/v0/tooling-access/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "endpoint_url": format!("{}/rpc", server.uri()),
                "token": "minted.jwt",
                "expires_at": FUTURE_ISO
            },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0xok"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    // Cache scoped to some other account's key hash.
    write_token_cache(&dir, &format!("{}/rpc", server.uri()), "deadbeef");

    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// A cached token pointing at an unreachable (disabled) endpoint yields a
// connect failure; the CLI probes status, sees disabled, clears the stale
// token, and routes into the same enable flow as the mint-400 path. In the
// harness (non-TTY + --no-input, no --yes) that flow can't prompt, so it ends
// in the actionable "run qn tooling-access enable" error.
#[tokio::test]
async fn connect_failure_with_disabled_status_prompts_to_enable() {
    let server = MockServer::start().await;
    // Status probe reports disabled.
    Mock::given(method("GET"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": false, "endpoint_url": null, "endpoint_id": 3 },
            "error": null
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    // Seed a still-valid token whose endpoint_url refuses connections fast
    // (loopback, almost-certainly-closed high port) so the RPC POST returns a
    // connect error promptly rather than waiting out the timeout.
    write_token_cache(&dir, "http://127.0.0.1:9/rpc", TEST_KEY_HASH);

    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber", "--retries", "0"],
    )
    .await;
    assert_ne!(out.exit_code, 0, "should fail");
    // Same actionable message as the mint-400 path (both converge on enable).
    assert!(
        out.stderr.contains("tooling-access enable"),
        "expected enable guidance, got: {}",
        out.stderr
    );
    // The stale token cache should have been cleared before the enable attempt.
    assert!(
        !dir.path().join("tokens.toml").exists(),
        "stale token cache should be cleared"
    );
}

// With --yes, the connect-failure-disabled path auto-enables and retries the
// call against the (now re-enabled) endpoint, minting a fresh token.
#[tokio::test]
async fn connect_failure_with_disabled_status_auto_enables_with_yes() {
    let server = MockServer::start().await;
    // Status reports disabled (drives the connect-failure path into enable).
    Mock::given(method("GET"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": false, "endpoint_url": null, "endpoint_id": 3 },
            "error": null
        })))
        .mount(&server)
        .await;
    // enable() PATCH succeeds.
    Mock::given(method("PATCH"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": true, "endpoint_url": format!("{}/rpc", server.uri()), "endpoint_id": 3 },
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    // After enabling, the retry re-mints (the stale token was cleared) and the
    // fresh token points at the reachable /rpc mock.
    Mock::given(method("POST"))
        .and(path("/v0/tooling-access/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "endpoint_url": format!("{}/rpc", server.uri()), "token": "minted.jwt", "expires_at": FUTURE_ISO },
            "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0xok"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, "http://127.0.0.1:9/rpc", TEST_KEY_HASH);

    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "eth_blockNumber", "--retries", "0", "--yes"],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// --network: status returns an id, get_endpoint_urls returns the per-network
// map, and the RPC call routes to the mapped (solana) URL, not the default.
#[tokio::test]
async fn network_routes_to_mapped_url() {
    let server = MockServer::start().await;
    let solana_url = format!("{}/solana", server.uri());

    Mock::given(method("GET"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": true, "endpoint_url": format!("{}/default", server.uri()), "endpoint_id": 3 },
            "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/3/urls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "http_url": format!("{}/default", server.uri()),
                "wss_url": null,
                "multichain_urls": {
                    "solana-mainnet": { "http_url": solana_url, "wss_url": null }
                }
            },
            "error": null
        })))
        .mount(&server)
        .await;
    // The call must hit /solana. /default is not mounted, so a misroute 404s.
    Mock::given(method("POST"))
        .and(path("/solana"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "12345"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, &format!("{}/default", server.uri()), TEST_KEY_HASH);

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "getSlot",
            "--network",
            "solana-mainnet",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    // networks.toml was written for reuse.
    let cached = std::fs::read_to_string(dir.path().join("networks.toml")).unwrap();
    assert!(cached.contains("solana-mainnet"), "cache: {cached}");
}

// --list-networks prints the available keys without making an RPC call.
#[tokio::test]
async fn list_networks_prints_keys() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": true, "endpoint_url": format!("{}/default", server.uri()), "endpoint_id": 3 },
            "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/3/urls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "http_url": format!("{}/default", server.uri()),
                "multichain_urls": {
                    "solana-mainnet": { "http_url": "https://x/sol", "wss_url": null },
                    "polygon": { "http_url": "https://x/matic", "wss_url": null }
                }
            },
            "error": null
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, &format!("{}/default", server.uri()), TEST_KEY_HASH);

    let out = run_qn(&server.uri(), &["--config-file", &cfg, "rpc", "--list-networks"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// Unknown --network surfaces an error (the SDK lists valid keys).
#[tokio::test]
async fn unknown_network_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enabled": true, "endpoint_url": format!("{}/default", server.uri()), "endpoint_id": 3 },
            "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v0/endpoints/3/urls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "http_url": format!("{}/default", server.uri()),
                "multichain_urls": {
                    "solana-mainnet": { "http_url": "https://x/sol", "wss_url": null }
                }
            },
            "error": null
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_path(&dir);
    write_token_cache(&dir, &format!("{}/default", server.uri()), TEST_KEY_HASH);

    let out = run_qn(
        &server.uri(),
        &["--config-file", &cfg, "rpc", "getSlot", "--network", "nope-mainnet"],
    )
    .await;
    assert_ne!(out.exit_code, 0, "should fail on unknown network");
    assert!(
        out.stderr.contains("unknown network") || out.stderr.contains("nope-mainnet"),
        "stderr={}",
        out.stderr
    );
}
