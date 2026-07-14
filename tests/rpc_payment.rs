//! Integration tests for the crypto-micropayment lane of `qn rpc call`
//! (`--x402`/`--mpp`).
//!
//! The harness's hidden `--base-url` is threaded into
//! `PaymentConfig.base_url_override`, so the mock server doubles as the
//! payment gateway: the paid lane POSTs to `{base}/<network>`. The 402
//! handshake shapes mirror the SDK's own driver tests (an unpaid POST gets a
//! 402 with a payment menu; the resend carries a `payment-signature` /
//! `authorization` header and gets the JSON-RPC result).
//!
//! All key material is fake: anvil throwaway key #0 (public, never funded)
//! and the public Base Sepolia test-USDC address.

mod common;

use common::{parse, run_qn, run_qn_no_key};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// anvil key #0 (public throwaway, never funded).
const EVM_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
// Base Sepolia test USDC (public address).
const USDC: &str = "0x036CbD53842c5426634e7929541eC2318f3dCF7e";

/// Writes the throwaway key to a tempfile and returns the (guard, path) pair.
fn key_file() -> (tempfile::NamedTempFile, String) {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(EVM_KEY.as_bytes()).unwrap();
    f.flush().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    (f, path)
}

/// One entry of the x402 payment menu the mock gateway offers.
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

/// Sequenced gateway responder: the first (unpaid) POST gets a 402 with a
/// one-entry menu at `amount`; any request carrying a payment signature gets
/// the `paid` response (the JSON-RPC result by default).
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

/// Mounts `.expect(0)` mocks on the control-plane routes the default lane
/// uses, proving the paid lane never touches them.
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

    // The harness injects --api-key test: even with a key present, the paid
    // lane must not mint, probe, or write the token cache.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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

    // `base-sepolia` resolves to eip155:84532 before reaching the SDK, so it
    // matches the mock gateway's CAIP-2 offer exactly.
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
            "--pay-network",
            "base-sepolia",
            "--asset",
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

    // Config supplies every parameter and has NO [api] key: the minimal
    // invocation is method + --network + the scheme flag, with no login ever.
    let dir = tempfile::tempdir().unwrap();
    let (_guard, key_path) = key_file();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[rpc.payment]\nkey_file = \"{key_path}\"\nmax_amount = \"10000\"\n\
             pay_network = \"eip155:84532\"\nasset = \"{USDC}\"\n"
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
    // Menu offers 999999; the cap is 10000. Exactly ONE request: the unpaid
    // probe. Nothing is signed, nothing is resent.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
    // Config would allow the 1000 offer (cap 999999); the flag caps at 1.
    // A resulting "unsupported" refusal proves the flag won.
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
             pay_network = \"eip155:84532\"\nasset = \"{USDC}\"\n"
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
    // Fully populated [rpc.payment] but NO scheme flag: the call must take the
    // normal Tooling Access lane (mint attempt against the control plane) and
    // the gateway must see zero traffic.
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
             pay_network = \"eip155:84532\"\nasset = \"{USDC}\"\n"
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
    // The default lane fails against the 500ing control plane — the exit code
    // is not the point; the .expect(0) on the gateway is.
    assert_ne!(out.exit_code, 0);
}

#[tokio::test]
async fn paid_lane_is_never_retried() {
    let server = MockServer::start().await;
    // The gateway 500s every request. Even with --retries 5, exactly ONE
    // request may arrive: a retried paid call risks a double charge.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
    // 402 menu, then the paid resend is refused with another 402: the gateway
    // refused the credential without settling it — exit 2, "refused", and no
    // unknown-outcome language.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
    // 402 menu, then the paid resend dies with a 500: the signed payment was
    // submitted and the outcome is unknown — exit 3, check-your-wallet.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
    // The paid resend returns 200 with a body that isn't JSON: the payment
    // was submitted (and likely settled) but the result is uninterpretable —
    // exit 3, never a generic decode error.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
    // The 402 challenge body is not JSON. Nothing has been signed or paid, so
    // this must exit 2 with "Nothing was charged" — never the exit-3
    // check-your-wallet path — after exactly one request.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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

/// Runs a paid invocation expected to die in pre-flight: any request reaching
/// the mock is a failure.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
            "--pay-network",
            "not-a-chain",
            "--asset",
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
    // No flag, no env in-process, and a config dir with no key_file.
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
            "--pay-network",
            "eip155:84532",
            "--asset",
            USDC,
            "--max-amount",
            "10000",
        ],
        "QN_PAYMENT_KEY",
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
         pay_network = \"eip155:84532\"\nasset = \"0xabc\"\n",
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
            "--pay-network",
            "eip155:84532",
            "--asset",
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
        vec!["--pay-network", "eip155:84532"],
        vec!["--asset", "0xabc"],
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

/// Runs the real binary against `server`, with HOME pointed at a tempdir so no
/// real config leaks in. The key is passed via QN_PAYMENT_KEY to also cover
/// env resolution (a subprocess env can't race parallel tests).
fn run_qn_subprocess(
    server_uri: &str,
    home: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .env_remove("HOME")
        .env("HOME", home)
        .env("QN_PAYMENT_KEY", EVM_KEY)
        .args(["--base-url", server_uri, "--no-input"])
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
async fn receipt_flag_wraps_stdout_on_mpp() {
    let server = MockServer::start().await;

    // MPP challenge (tempo, chain 42431) and a settlement receipt header, both
    // base64url-encoded JSON — mirrors the SDK's own MPP driver test.
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
            "--pay-network",
            "eip155:42431",
            "--asset",
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
    // The raw key must never appear on either stream.
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
        "--pay-network",
        "eip155:84532",
        "--asset",
        USDC,
        "--max-amount",
        "10000",
    ];

    // With --receipt: wrapped, and x402 has no settlement reference.
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

    // Without --receipt: the bare result, byte-identical shape to unpaid.
    let output = run_qn_subprocess(&server.uri(), home.path(), &paid_args);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v.as_str(), Some("0x1335f9a"));
}
