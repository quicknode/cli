//! Integration tests for paid RPC calls and payment lifecycle commands.

mod common;

use common::{parse, run_qn, run_qn_no_key};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const EVM_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const USDC: &str = "0x036CbD53842c5426634e7929541eC2318f3dCF7e";

/// Write the test key to a temporary file.
fn key_file() -> (tempfile::NamedTempFile, String) {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(EVM_KEY.as_bytes()).unwrap();
    f.flush().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    (f, path)
}

/// Build one x402 offer.
fn x402_accepts_entry(amount: &str) -> serde_json::Value {
    json!({
        "scheme": "exact",
        "network": "eip155:84532",
        "amount": amount,
        "payTo": "0x000000000000000000000000000000000000dEaD",
        "maxTimeoutSeconds": 60,
        "asset": USDC,
        "extra": { "name": "USDC", "version": "2" }
    })
}

/// Return a 402 offer, then a paid response.
struct X402Seq {
    amount: &'static str,
    paid: ResponseTemplate,
    calls: AtomicUsize,
}

impl X402Seq {
    fn new(amount: &'static str) -> Self {
        Self::with_paid_response(
            amount,
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": "0x1335f9a"
            })),
        )
    }

    fn with_paid_response(amount: &'static str, paid: ResponseTemplate) -> Self {
        Self {
            amount,
            paid,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Respond for X402Seq {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let has_sig = req.headers.contains_key("payment-signature");
        if n == 0 && !has_sig {
            ResponseTemplate::new(402).set_body_json(json!({
                "x402Version": 2,
                "accepts": [ x402_accepts_entry(self.amount) ]
            }))
        } else {
            self.paid.clone()
        }
    }
}

/// Assert that the paid lane does not use control-plane routes.
async fn mount_control_plane_expect_zero(server: &MockServer) {
    for p in ["/v0/tooling-access", "/v0/account/info"] {
        Mock::given(path(p))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(server)
            .await;
    }
}

// ── happy paths ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn x402_happy_path_pays_and_bypasses_control_plane() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .expect(2) // one unpaid probe + one paid resend, nothing else
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    assert!(
        !dir.path().join("tokens.toml").exists(),
        "paid lane must not write the token cache"
    );
}

#[tokio::test]
async fn x402_resolves_usdc_symbol_to_network_asset() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .expect(2) // one unpaid probe + one paid resend
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            "USDC",
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn payment_wallet_resolves_stored_key_for_paid_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .expect(2) // one unpaid probe + one paid resend
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();

    let gen = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "wallet",
            "generate",
            "--vm",
            "evm",
            "--name",
            "payer",
        ],
    )
    .await;
    assert_eq!(gen.exit_code, 0, "stderr={}", gen.stderr);

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-wallet",
            "payer",
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn pay_network_name_matches_caip2_offer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .expect(2)
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "base-sepolia",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn paid_call_works_keyless_with_config_params() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .expect(2)
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let (_guard, key_path) = key_file();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[rpc.payment]\nkey_file = \"{key_path}\"\nmax_amount = \"10000\"\n\
             payment_network = \"eip155:84532\"\npayment_asset = \"{USDC}\"\n"
        ),
    )
    .unwrap();

    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            cfg.to_str().unwrap(),
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

// ── spend-cap enforcement ────────────────────────────────────────────────────

#[tokio::test]
async fn over_cap_offer_is_refused_before_signing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("999999"))
        .expect(1)
        .mount(&server)
        .await;

    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("Nothing was charged"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn flag_max_amount_overrides_config() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let (_guard, key_path) = key_file();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[rpc.payment]\nkey_file = \"{key_path}\"\nmax_amount = \"999999\"\n\
             payment_network = \"eip155:84532\"\npayment_asset = \"{USDC}\"\n"
        ),
    )
    .unwrap();

    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            cfg.to_str().unwrap(),
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--max-amount",
            "1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("Nothing was charged"),
        "stderr={}",
        out.stderr
    );
}

// ── activation and lane isolation ────────────────────────────────────────────

#[tokio::test]
async fn config_presence_does_not_auto_activate_payment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let (_guard, key_path) = key_file();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[api]\nkey = \"test\"\n\n\
             [rpc.payment]\nkey_file = \"{key_path}\"\nmax_amount = \"10000\"\n\
             payment_network = \"eip155:84532\"\npayment_asset = \"{USDC}\"\n"
        ),
    )
    .unwrap();

    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            cfg.to_str().unwrap(),
            "rpc",
            "call",
            "eth_blockNumber",
        ],
    )
    .await;
    assert_ne!(out.exit_code, 0);
}

#[tokio::test]
async fn paid_lane_is_never_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .expect(1)
        .mount(&server)
        .await;

    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "--retries",
            "5",
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_ne!(out.exit_code, 0);
}

#[tokio::test]
async fn payment_rejected_on_paid_resend_exits_2_as_refused() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "x402Version": 2,
            "accepts": [ x402_accepts_entry("1000") ]
        })))
        .expect(2)
        .mount(&server)
        .await;

    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(out.stderr.contains("refused"), "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("nothing should have settled"),
        "stderr={}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("may have been settled"),
        "a 4xx refusal must not carry unknown-outcome language: {}",
        out.stderr
    );
}

#[tokio::test]
async fn settlement_5xx_on_paid_resend_exits_3_check_wallet() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::with_paid_response(
            "1000",
            ResponseTemplate::new(500).set_body_string("settlement error"),
        ))
        .expect(2)
        .mount(&server)
        .await;

    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 3, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("check your wallet"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn unparseable_paid_response_exits_3_check_wallet() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::with_paid_response(
            "1000",
            ResponseTemplate::new(200).set_body_string("<html>ok?</html>"),
        ))
        .expect(2)
        .mount(&server)
        .await;

    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 3, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("may have been settled"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn malformed_challenge_menu_exits_2_nothing_charged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(402).set_body_string("<html>menu?</html>"))
        .expect(1)
        .mount(&server)
        .await;

    let (_guard, key_path) = key_file();
    let out = run_qn(
        &server.uri(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("Nothing was charged"),
        "stderr={}",
        out.stderr
    );
}

// ── pre-flight failures: fail fast, zero requests ────────────────────────────

/// Run a paid invocation that must fail before network I/O.
async fn expect_preflight_error(extra: &[&str], needle: &str) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let out = run_qn(&server.uri(), extra).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains(needle),
        "expected {needle:?} in stderr: {}",
        out.stderr
    );
}

#[tokio::test]
async fn missing_network_fails_before_any_request() {
    let (_guard, key_path) = key_file();
    expect_preflight_error(
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
        "--network",
    )
    .await;
}

#[tokio::test]
async fn unknown_pay_network_name_fails_before_any_request() {
    let (_guard, key_path) = key_file();
    expect_preflight_error(
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "not-a-chain",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
        "unknown pay network 'not-a-chain'",
    )
    .await;
}

#[tokio::test]
async fn missing_key_fails_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    expect_preflight_error(
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
        "--payment-key-file",
    )
    .await;
}

#[tokio::test]
async fn missing_max_amount_fails_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    expect_preflight_error(
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
        ],
        "--max-amount",
    )
    .await;
}

#[tokio::test]
async fn non_integer_max_amount_fails_before_any_request() {
    let (_guard, key_path) = key_file();
    expect_preflight_error(
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "1.5",
        ],
        "base units",
    )
    .await;
}

#[tokio::test]
async fn inline_config_key_fails_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        "[rpc.payment]\nkey = \"0xdeadbeef\"\nmax_amount = \"10000\"\n\
         payment_network = \"eip155:84532\"\npayment_asset = \"0xabc\"\n",
    )
    .unwrap();
    expect_preflight_error(
        &[
            "--config-file",
            cfg.to_str().unwrap(),
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402",
        ],
        "key_file",
    )
    .await;
}

#[tokio::test]
async fn params_and_key_both_from_stdin_conflict() {
    let (_guard, _) = key_file();
    expect_preflight_error(
        &[
            "rpc",
            "call",
            "eth_call",
            "-",
            "--network",
            "base-sepolia",
            "--x402",
            "--payment-key-file",
            "-",
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000",
        ],
        "stdin",
    )
    .await;
}

// ── clap surface ─────────────────────────────────────────────────────────────

#[test]
fn x402_and_mpp_are_mutually_exclusive() {
    let err = parse(&[
        "rpc",
        "call",
        "eth_blockNumber",
        "--network",
        "n",
        "--x402",
        "--mpp",
    ])
    .unwrap_err();
    assert!(err.to_string().contains("--mpp"), "got: {err}");
}

#[test]
fn payment_flags_require_a_scheme_flag() {
    for orphan in [
        vec!["--receipt"],
        vec!["--payment-network", "eip155:84532"],
        vec!["--payment-asset", "0xabc"],
        vec!["--max-amount", "1"],
        vec!["--payment-key-file", "/k"],
        vec!["--svm-rpc-url", "https://x"],
    ] {
        let mut argv = vec!["rpc", "call", "eth_blockNumber"];
        argv.extend(orphan.iter().copied());
        assert!(
            parse(&argv).is_err(),
            "expected {orphan:?} to require --x402/--mpp"
        );
    }
}

#[test]
fn payment_conflicts_with_endpoint_url() {
    for scheme in ["--x402", "--mpp"] {
        let err = parse(&[
            "rpc",
            "call",
            "eth_blockNumber",
            scheme,
            "--endpoint-url",
            "https://x/rpc",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("--endpoint-url"), "got: {err}");
    }
}

// ── output shapes (subprocess: the in-process harness can't capture stdout) ──

/// Run the real binary with an isolated home and key file.
fn run_qn_subprocess(
    server_uri: &str,
    home: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    let key_path = home.join("payer.key");
    std::fs::write(&key_path, EVM_KEY).unwrap();
    assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .env_remove("HOME")
        .env("HOME", home)
        .args(["--base-url", server_uri, "--no-input"])
        .args(args)
        .args(["--payment-key-file", key_path.to_str().unwrap()])
        .output()
        .unwrap()
}

#[tokio::test]
async fn receipt_flag_wraps_stdout_on_mpp() {
    let server = MockServer::start().await;

    fn b64url(v: serde_json::Value) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&v).unwrap())
    }
    let request = b64url(json!({
        "amount": "1000",
        "currency": "0x20c0000000000000000000000000000000000000",
        "recipient": "0x000000000000000000000000000000000000bEEF",
        "methodDetails": { "chainId": 42431, "feePayer": true }
    }));
    let www = format!(
        "Payment id=\"c1\", realm=\"mpp.example.com\", method=\"tempo\", \
         intent=\"charge\", description=\"d\", expires=\"2099-01-01T00:00:00Z\", \
         request=\"{request}\""
    );
    let receipt = b64url(json!({
        "method": "tempo", "status": "success",
        "timestamp": "2026-01-01T00:00:00Z",
        "reference": "0xdeadbeef"
    }));

    struct MppSeq {
        www: String,
        receipt: String,
        calls: AtomicUsize,
    }
    impl Respond for MppSeq {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 && !req.headers.contains_key("authorization") {
                ResponseTemplate::new(402)
                    .insert_header("WWW-Authenticate", self.www.as_str())
                    .set_body_json(json!({ "type": "about:blank" }))
            } else {
                ResponseTemplate::new(200)
                    .insert_header("Payment-Receipt", self.receipt.as_str())
                    .set_body_json(json!({ "jsonrpc": "2.0", "id": 1, "result": "0xok" }))
            }
        }
    }
    Mock::given(method("POST"))
        .and(path("/tempo-testnet"))
        .respond_with(MppSeq {
            www,
            receipt,
            calls: AtomicUsize::new(0),
        })
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let output = run_qn_subprocess(
        &server.uri(),
        home.path(),
        &[
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "tempo-testnet",
            "--mpp",
            "--receipt",
            "--payment-network",
            "eip155:42431",
            "--payment-asset",
            "0x20c0000000000000000000000000000000000000",
            "--max-amount",
            "10000",
        ],
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "stderr={stderr}");

    let v: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("bad JSON: {e}\n{stdout}"));
    assert_eq!(v["result"].as_str(), Some("0xok"));
    assert_eq!(
        v["payment_receipt"]["reference"].as_str(),
        Some("0xdeadbeef")
    );
    assert!(!stdout.contains(EVM_KEY) && !stderr.contains(EVM_KEY));
}

#[tokio::test]
async fn receipt_is_null_on_x402_and_bare_without_flag() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(X402Seq::new("1000"))
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let paid_args = [
        "rpc",
        "call",
        "eth_blockNumber",
        "--network",
        "base-sepolia",
        "--x402",
        "--payment-network",
        "eip155:84532",
        "--payment-asset",
        USDC,
        "--max-amount",
        "10000",
    ];

    let mut with_receipt = paid_args.to_vec();
    with_receipt.push("--receipt");
    let output = run_qn_subprocess(&server.uri(), home.path(), &with_receipt);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["result"].as_str(), Some("0x1335f9a"));
    assert!(v["payment_receipt"].is_null());

    let output = run_qn_subprocess(&server.uri(), home.path(), &paid_args);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v.as_str(), Some("0x1335f9a"));
}

// ── qn rpc x402 (credit drawdown lifecycle) ──────────────────────────────────
//

/// Mount a fixed SIWX session response.
async fn mount_auth(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "jwt-test",
            "expiresAt": "2099-01-01T00:00:00Z",
            "accountId": "eip155:84532:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        })))
        .mount(server)
        .await;
}

fn x402_args<'a>(cfg: &'a str, key_path: &'a str, verb: &'a str) -> Vec<&'a str> {
    vec![
        "--config-file",
        cfg,
        "rpc",
        "x402",
        verb,
        "--payment-key-file",
        key_path,
        "--payment-network",
        "eip155:84532",
        "--payment-asset",
        USDC,
        "--max-amount",
        "10000000",
    ]
}

fn x402_session_args<'a>(cfg: &'a str, key_path: &'a str, verb: &'a str) -> Vec<&'a str> {
    vec![
        "--config-file",
        cfg,
        "rpc",
        "x402",
        verb,
        "--payment-key-file",
        key_path,
        "--payment-network",
        "eip155:84532",
    ]
}

#[tokio::test]
async fn x402_buy_credits_selects_the_regular_credit_tier() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    let mut nanopayment = x402_accepts_entry("100");
    nanopayment["maxTimeoutSeconds"] = json!(604_900);
    nanopayment["extra"] = json!({
        "name": "GatewayWalletBatched",
        "version": "1",
        "verifyingContract": "0x0077777d7EBA4688BDeF3E311b846F25870A19B9"
    });
    struct BuySeq {
        calls: AtomicUsize,
        nanopayment: serde_json::Value,
    }
    impl Respond for BuySeq {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 && !req.headers.contains_key("payment-signature") {
                ResponseTemplate::new(402).set_body_json(json!({
                    "x402Version": 2,
                    "accepts": [x402_accepts_entry("1000000"), x402_accepts_entry("1000"), self.nanopayment.clone()],
                }))
            } else {
                use base64::Engine;
                let header = req
                    .headers
                    .get("payment-signature")
                    .expect("paid resend must include a payment signature")
                    .to_str()
                    .unwrap();
                let envelope: serde_json::Value = serde_json::from_slice(
                    &base64::engine::general_purpose::STANDARD
                        .decode(header)
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(envelope["accepted"]["amount"], "1000000");
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1, "result": "0x1"
                }))
            }
        }
    }
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(BuySeq {
            calls: AtomicUsize::new(0),
            nanopayment,
        })
        .expect(2) // one offer probe and one paid resend
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "eip155:84532:0xabc", "credits": 100_000u64
        })))
        .expect(1)
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut args = x402_args(&cfg, &key_path, "buy-credits");
    args.extend_from_slice(&["--network", "base-sepolia", "--yes"]); // query chain + consent
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_buy_credits_without_yes_is_needs_confirmation_and_settles_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let args = x402_args(&cfg, &key_path, "buy-credits");
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_balance_prints_credits() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    Mock::given(method("GET"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "eip155:84532:0xabc", "credits": 42u64
        })))
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &x402_session_args(&cfg, &key_path, "balance"),
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_balance_error_maps_to_exit_2() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    Mock::given(method("GET"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_token", "message": "session token invalid"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &x402_session_args(&cfg, &key_path, "balance"),
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_drip_reports_funding_tx() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    Mock::given(method("POST"))
        .and(path("/drip"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "eip155:84532:0xabc",
            "walletAddress": "0xabc",
            "transactionHash": "0xfeed"
        })))
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(&server.uri(), &x402_session_args(&cfg, &key_path, "drip")).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_balance_rejects_spend_flags() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut args = x402_session_args(&cfg, &key_path, "balance");
    args.extend_from_slice(&["--max-amount", "10000000"]);
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

// ── qn rpc call --x402-drawdown ──────────────────────────────────────────────
//

fn drawdown_call_args<'a>(cfg: &'a str, key_path: &'a str) -> Vec<&'a str> {
    vec![
        "--config-file",
        cfg,
        "rpc",
        "call",
        "eth_blockNumber",
        "--network",
        "base-sepolia",
        "--x402-drawdown",
        "--payment-key-file",
        key_path,
        "--payment-network",
        "eip155:84532",
        "--payment-asset",
        USDC,
        "--max-amount",
        "10000000",
    ]
}

#[tokio::test]
async fn x402_drawdown_happy_path_uses_bearer_no_signing() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0x1335f9a"
        })))
        .expect(1) // single attempt, no per-call 402 handshake
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(&server.uri(), &drawdown_call_args(&cfg, &key_path)).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    assert!(!dir.path().join("tokens.toml").exists());
    assert!(dir.path().join("sessions.toml").exists());
}

#[tokio::test]
async fn x402_drawdown_needs_only_wallet_and_network() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0x1335f9a"
        })))
        .expect(1)
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "base-sepolia",
            "--x402-drawdown",
            "--payment-key-file",
            &key_path,
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

/// Return an expired-token response once, then success.
struct ExpiredThenOk {
    status: u16,
    calls: AtomicUsize,
}

impl Respond for ExpiredThenOk {
    fn respond(&self, _req: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            ResponseTemplate::new(self.status).set_body_json(json!({
                "error": "token_expired", "message": "session token expired"
            }))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": "0xafterreauth"
            }))
        }
    }
}

async fn assert_drawdown_reauths_on(status: u16) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "jwt-test",
            "expiresAt": "2099-01-01T00:00:00Z",
            "accountId": "eip155:84532:0xabc"
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ExpiredThenOk {
            status,
            calls: AtomicUsize::new(0),
        })
        .expect(2) // one expired attempt + one retry after re-auth
        .mount(&server)
        .await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(&server.uri(), &drawdown_call_args(&cfg, &key_path)).await;
    assert_eq!(out.exit_code, 0, "status={status} stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_drawdown_reauths_once_on_token_expired_401() {
    assert_drawdown_reauths_on(401).await;
}

#[tokio::test]
async fn x402_drawdown_reauths_once_on_token_expired_403() {
    assert_drawdown_reauths_on(403).await;
}

#[tokio::test]
async fn x402_drawdown_rejects_key_and_params_both_from_stdin() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_call",
            "-", // params from stdin
            "--network",
            "base-sepolia",
            "--x402-drawdown",
            "--payment-key-file",
            "-", // key ALSO from stdin
            "--payment-network",
            "eip155:84532",
            "--payment-asset",
            USDC,
            "--max-amount",
            "10000000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("both") && out.stderr.contains("stdin"),
        "expected the both-from-stdin guard, got: {}",
        out.stderr
    );
}

#[tokio::test]
async fn x402_drawdown_out_of_credits_points_at_buy_credits() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": "insufficient_credits", "message": "no credits remaining"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(&server.uri(), &drawdown_call_args(&cfg, &key_path)).await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("buy-credits"),
        "stderr should point at buy-credits, got: {}",
        out.stderr
    );
}

// ── qn rpc mpp (payment channel session) ─────────────────────────────────────
//

use base64::Engine as _;

const SESSION_ESCROW: &str = "0x33b901018174ddabe4841042ab76ba85d4e24f25";

fn session_request_b64() -> String {
    let json = json!({
        "amount": "500",
        "currency": "0x20c0000000000000000000000000000000000000",
        "recipient": "0xfd24114c3981aba78ae2441991b1bdb89329c556",
        "methodDetails": { "chainId": 42431, "escrowContract": SESSION_ESCROW }
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap())
}

fn session_www_authenticate() -> String {
    format!(
        "Payment id=\"c1\", realm=\"mpp.quicknode.com\", method=\"tempo\", intent=\"session\", description=\"d\", expires=\"2099-01-01T00:00:00Z\", request=\"{}\"",
        session_request_b64()
    )
}

fn session_receipt_b64(accepted: &str, spent: &str) -> String {
    let json = json!({
        "method": "tempo",
        "intent": "session",
        "status": "success",
        "timestamp": "2099-01-01T00:00:00Z",
        "reference": format!("0x{}", "ab".repeat(32)),
        "challengeId": "c1",
        "channelId": format!("0x{}", "ab".repeat(32)),
        "acceptedCumulative": accepted,
        "spent": spent,
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap())
}

async fn mount_session(server: &MockServer, network: &str) {
    let path_str = format!("/session/{network}");
    Mock::given(method("POST"))
        .and(path(path_str.clone()))
        .and(wiremock::matchers::header_exists("authorization"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "Payment-Receipt",
                    session_receipt_b64("500", "500").as_str(),
                )
                .set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1, "result": "0xok"
                })),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(path_str))
        .respond_with(
            ResponseTemplate::new(402)
                .insert_header("WWW-Authenticate", session_www_authenticate().as_str())
                .set_body_json(json!({ "type": "about:blank" })),
        )
        .mount(server)
        .await;
}

fn credential_payload(req: &wiremock::Request) -> serde_json::Value {
    let header = req
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .expect("credential POST must carry Authorization");
    let b64 = header
        .strip_prefix("Payment ")
        .expect("Authorization must be a Payment credential");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64)
        .expect("credential must be base64url");
    let credential: serde_json::Value =
        serde_json::from_slice(&bytes).expect("credential must be JSON");
    credential["payload"].clone()
}

fn mpp_args<'a>(cfg: &'a str, key_path: &'a str, verb: &'a str) -> Vec<&'a str> {
    vec![
        "--config-file",
        cfg,
        "rpc",
        "mpp",
        verb,
        "--payment-key-file",
        key_path,
        "--payment-network",
        "eip155:42431",
        "--payment-asset",
        "0x20c0000000000000000000000000000000000000",
        "--max-amount",
        "100000000",
    ]
}

#[tokio::test]
async fn mpp_open_happy_path_and_caches_channel() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut args = mpp_args(&cfg, &key_path, "open");
    args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    assert!(
        dir.path().join("channels.toml").exists(),
        "open must cache the channel state"
    );

    let requests = server.received_requests().await.unwrap();
    let payload = requests
        .iter()
        .find(|r| r.headers.contains_key("authorization"))
        .map(credential_payload)
        .expect("an open credential must have been POSTed");
    assert_eq!(payload["action"], "open");
    assert_eq!(payload["type"], "transaction");
    assert_eq!(payload["cumulativeAmount"], "500");
    assert!(payload.get("descriptor").is_none(), "got: {payload}");
    for key in ["channelId", "transaction", "signature", "authorizedSigner"] {
        assert!(
            payload[key].as_str().is_some_and(|s| s.starts_with("0x")),
            "payload must carry hex {key}: {payload}"
        );
    }
}

#[tokio::test]
async fn mpp_status_replays_voucher_and_reads_the_receipt() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let before = server.received_requests().await.unwrap().len();
    let out = run_qn(&server.uri(), &mpp_args(&cfg, &key_path, "status")).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    let after = server.received_requests().await.unwrap().len();
    assert_eq!(after, before, "status without --verify must not call out");

    let mut verify_args = mpp_args(&cfg, &key_path, "status");
    verify_args.push("--verify");
    let out = run_qn(&server.uri(), &verify_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let requests = server.received_requests().await.unwrap();
    let voucher = requests
        .iter()
        .filter(|r| r.headers.contains_key("authorization"))
        .map(credential_payload)
        .find(|p| p["action"] == "voucher")
        .expect("status --verify must POST a voucher credential");
    assert_eq!(voucher["cumulativeAmount"], "1000");
    assert!(voucher.get("descriptor").is_none(), "got: {voucher}");
}

#[tokio::test]
async fn mpp_open_without_yes_is_needs_confirmation_and_settles_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/session/tempo-testnet"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut args = mpp_args(&cfg, &key_path, "open");
    args.extend_from_slice(&["--deposit", "1000000"]);
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
}

#[tokio::test]
async fn mpp_top_up_without_yes_is_needs_confirmation_and_settles_nothing() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let before = server.received_requests().await.unwrap().len();
    let mut args = mpp_args(&cfg, &key_path, "top-up");
    args.extend_from_slice(&["--deposit", "500000"]);
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
    let after = server.received_requests().await.unwrap().len();
    assert_eq!(after, before, "a refused top-up must not settle anything");
}

#[tokio::test]
async fn mpp_top_up_with_yes_adds_deposit_to_the_open_channel() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let mut args = mpp_args(&cfg, &key_path, "top-up");
    args.extend_from_slice(&["--deposit", "500000", "--yes"]);
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn mpp_close_without_yes_is_needs_confirmation_and_settles_nothing() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let before = server.received_requests().await.unwrap().len();
    let out = run_qn(&server.uri(), &mpp_args(&cfg, &key_path, "close")).await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
    let after = server.received_requests().await.unwrap().len();
    assert_eq!(after, before, "a refused close must not settle anything");
    assert!(
        dir.path().join("channels.toml").exists(),
        "a refused close must leave the channel record intact"
    );
}

#[tokio::test]
async fn mpp_close_with_yes_settles_the_channel() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let mut args = mpp_args(&cfg, &key_path, "close");
    args.push("--yes");
    let out = run_qn(&server.uri(), &args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn mpp_session_call_without_open_channel_points_at_open() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/session/tempo-testnet"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "tempo-testnet",
            "--mpp-session",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:42431",
            "--payment-asset",
            "0x20c0000000000000000000000000000000000000",
            "--max-amount",
            "100000000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("mpp open"),
        "should point at 'mpp open', got: {}",
        out.stderr
    );
}

#[tokio::test]
async fn mpp_session_call_uses_the_channel_for_a_different_query_network() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_session(&server, "ethereum-mainnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "eth_blockNumber",
            "--network",
            "ethereum-mainnet",
            "--mpp-session",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:42431",
            "--payment-asset",
            "0x20c0000000000000000000000000000000000000",
            "--max-amount",
            "100000000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let requests = server.received_requests().await.unwrap();
    let voucher = requests
        .iter()
        .filter(|r| r.url.path() == "/session/ethereum-mainnet")
        .filter(|r| r.headers.contains_key("authorization"))
        .map(credential_payload)
        .find(|p| p["action"] == "voucher")
        .expect("the mainnet call must POST a voucher credential");
    assert_eq!(voucher["cumulativeAmount"], "1000");
}

#[tokio::test]
async fn mpp_channels_do_not_collide_across_pay_assets() {
    let server = MockServer::start().await;
    mount_session(&server, "tempo-testnet").await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut open_args = mpp_args(&cfg, &key_path, "open");
    open_args.extend_from_slice(&["--deposit", "1000000", "--yes"]);
    let out = run_qn(&server.uri(), &open_args).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "mpp",
            "status",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "eip155:42431",
            "--payment-asset",
            "0x20c0000000000000000000000000000000000001",
            "--max-amount",
            "100000000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("mpp open"),
        "should point at 'mpp open', got: {}",
        out.stderr
    );
}

#[tokio::test]
async fn mpp_lifecycle_verbs_reject_a_query_network() {
    let server = MockServer::start().await;
    mount_control_plane_expect_zero(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    for verb in ["open", "top-up", "close", "status"] {
        let mut args = mpp_args(&cfg, &key_path, verb);
        args.extend_from_slice(&["--network", "tempo-testnet"]);
        if verb == "open" || verb == "top-up" {
            args.extend_from_slice(&["--deposit", "1000000"]);
        }
        let out = run_qn(&server.uri(), &args).await;
        assert_ne!(out.exit_code, 0, "{verb} should reject --network");
        assert!(
            out.stderr.contains("unexpected argument '--network'"),
            "{verb} should reject --network as unknown, got: {}",
            out.stderr
        );
    }
}

// ── x402 / Solana ────────────────────────────────────────────────────────────

const SOL_MINT: &str = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
const SOL_FEE_PAYER: &str = "CPZSjRmyfTS95UjQD8ZdeTEWbQvW9QvEXnn6aGP7yyMN";
const SOL_PAY_TO: &str = "2LWbc9Mi6dRUrdEHBttoNS4udDtH1A4xwBdm1EKqcT57";
const SOL_DEVNET: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

/// Build a Solana x402 offer.
fn x402_solana_entry(amount: &str) -> serde_json::Value {
    json!({
        "scheme": "exact",
        "network": SOL_DEVNET,
        "amount": amount,
        "payTo": SOL_PAY_TO,
        "maxTimeoutSeconds": 60,
        "asset": SOL_MINT,
        "extra": { "feePayer": SOL_FEE_PAYER }
    })
}

struct SolanaSeq {
    calls: AtomicUsize,
    envelope: std::sync::Mutex<Option<serde_json::Value>>,
}

impl SolanaSeq {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            envelope: std::sync::Mutex::new(None),
        }
    }

    fn recorded_envelope(&self) -> Option<serde_json::Value> {
        self.envelope.lock().unwrap().clone()
    }
}

struct SharedSolanaSeq(std::sync::Arc<SolanaSeq>);

impl Respond for SharedSolanaSeq {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.0.respond(req)
    }
}

impl SolanaSeq {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
        match body.get("method").and_then(|m| m.as_str()) {
            Some("getLatestBlockhash") => {
                return ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "value": { "blockhash": "11111111111111111111111111111112" } }
                }));
            }
            Some("getAccountInfo") => {
                return ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "value": {
                        "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
                        "data": { "parsed": { "info": { "decimals": 6 } } }
                    } }
                }));
            }
            _ => {}
        }

        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        match req.headers.get("payment-signature") {
            None if n == 0 => ResponseTemplate::new(402).set_body_json(json!({
                "x402Version": 2,
                "accepts": [x402_solana_entry("1000000"), x402_solana_entry("1000")]
            })),
            Some(sig) => {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(sig.to_str().unwrap())
                    .expect("payment-signature must be base64");
                *self.envelope.lock().unwrap() =
                    Some(serde_json::from_slice(&decoded).expect("envelope must be JSON"));
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1, "result": 1234
                }))
            }
            None => ResponseTemplate::new(500),
        }
    }
}

#[tokio::test]
async fn x402_solana_pays_the_cheapest_offer_with_a_transaction_payload() {
    let server = MockServer::start().await;
    let seq = std::sync::Arc::new(SolanaSeq::new());
    Mock::given(method("POST"))
        .respond_with(SharedSolanaSeq(seq.clone()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();

    let gen = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "wallet",
            "generate",
            "--vm",
            "svm",
            "--name",
            "sol-payer",
        ],
    )
    .await;
    assert_eq!(gen.exit_code, 0, "stderr={}", gen.stderr);

    let out = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "rpc",
            "call",
            "getSlot",
            "--network",
            "solana-devnet",
            "--x402",
            "--payment-wallet",
            "sol-payer",
            "--payment-network",
            "solana-devnet",
            "--payment-asset",
            SOL_MINT,
            "--max-amount",
            "1000000",
            "--svm-rpc-url",
            &server.uri(),
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let envelope = seq
        .recorded_envelope()
        .expect("the paid resend must carry a payment-signature");

    let tx = envelope
        .pointer("/payload/transaction")
        .and_then(|t| t.as_str())
        .expect("payload.transaction must be a base64 string");
    assert!(!tx.is_empty(), "transaction must not be empty");

    assert_eq!(
        envelope
            .pointer("/accepted/amount")
            .and_then(|a| a.as_str()),
        Some("1000"),
        "must pay the cheapest offer, not the first listed"
    );
    assert_eq!(
        envelope.get("x402Version").and_then(|v| v.as_u64()),
        Some(2)
    );
}
