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

    // The symbol resolves to base-sepolia's USDC address (matching the offer's
    // `asset`), so signing and the paid resend succeed exactly as passing the
    // raw address would.
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

    // Generate a wallet, then pay for a call by referencing it by name. The
    // wallet's key is random but a valid secp256k1 key, so the x402 EIP-712
    // signing succeeds and the resend carries a payment signature.
    let gen = run_qn(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "wallet",
            "generate",
            "--chain",
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

    // Config supplies every parameter and has NO [api] key: the minimal
    // invocation is method + --network + the scheme flag, with no login ever.
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
    // No flag and a config dir with no key_file/wallet: the key must come from
    // a file or a stored wallet, so this fails fast with actionable guidance.
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

/// Runs the real binary against `server`, with HOME pointed at a tempdir so no
/// real config leaks in. The payment key is written to a file under `home` and
/// passed via `--payment-key-file` (the key never comes from the environment).
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
        "--payment-network",
        "eip155:84532",
        "--payment-asset",
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

// ── qn rpc x402 (credit drawdown lifecycle) ──────────────────────────────────
//
// These exercise the x402 noun end-to-end against the mock gateway: SIWX auth
// (POST /auth) mints a session JWT, buy-credits settles the 402 credit offer
// (POST /credits), balance reads GET /credits, and drip hits POST /drip. The
// session is cached under the config dir so a second verb skips re-auth.

/// Mounts a SIWX /auth responder that returns a fixed session JWT.
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

/// Sequenced network-scoped credit-purchase responder: the first (unpaid) POST
/// gets a 402 credit offer; the paid resend (with a payment signature) gets a
/// 200 RPC result. The funded balance is read separately via GET /credits.
struct CreditsSeq {
    amount: &'static str,
    calls: AtomicUsize,
}

impl Respond for CreditsSeq {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let has_sig = req.headers.contains_key("payment-signature");
        if n == 0 && !has_sig {
            ResponseTemplate::new(402).set_body_json(json!({
                "x402Version": 2,
                "accepts": [ x402_accepts_entry(self.amount) ]
            }))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": "0x1"
            }))
        }
    }
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

// The session verbs (balance, drip) sign nothing, so they take only the wallet
// key + pay network — no --payment-asset, no --max-amount.
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
async fn x402_buy_credits_happy_path() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    // Credits are bought by settling the offer on the network-scoped RPC path.
    Mock::given(method("POST"))
        .and(path("/base-sepolia"))
        .respond_with(CreditsSeq {
            amount: "1000000",
            calls: AtomicUsize::new(0),
        })
        .expect(2) // one unpaid offer probe + one paid resend
        .mount(&server)
        .await;
    // The funded balance is then read from GET /credits.
    Mock::given(method("GET"))
        .and(path("/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "eip155:84532:0xabc", "credits": 1_000_095u64
        })))
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
    // The session JWT is cached for reuse.
    assert!(dir.path().join("sessions.toml").exists());
}

#[tokio::test]
async fn x402_buy_credits_without_yes_is_needs_confirmation_and_settles_nothing() {
    let server = MockServer::start().await;
    // The gate is checked before any network I/O: nothing must reach the gateway.
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

    // No --yes, non-TTY (the harness sets --no-input): exit 5, zero requests.
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
    // A gateway 4xx that settled nothing maps to exit 2 (SDK Api error).
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
}

#[tokio::test]
async fn x402_drip_reports_funding_tx() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    // The faucet returns the on-chain funding transaction, not a credit balance.
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
    // balance signs nothing, so --max-amount (and --payment-asset) are not part
    // of its surface: clap rejects the unknown flag before any I/O.
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();

    let mut args = x402_session_args(&cfg, &key_path, "balance");
    args.extend_from_slice(&["--max-amount", "10000000"]);
    let out = run_qn(&server.uri(), &args).await;
    // Unknown flag: clap usage error, exit 1.
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
}

// ── qn rpc call --x402-drawdown ──────────────────────────────────────────────
//
// The drawdown lane pays from prepaid credits: no per-call signing, Bearer JWT,
// 1 credit per success. The session is authenticated once (POST /auth), and a
// token_expired 401 triggers exactly one transparent re-auth + retry.

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
    // The drawdown POST carries a Bearer JWT and NO payment-signature.
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
    // No token cache written (keyless lane), but the gateway session is cached.
    assert!(!dir.path().join("tokens.toml").exists());
    assert!(dir.path().join("sessions.toml").exists());
}

#[tokio::test]
async fn x402_drawdown_needs_only_wallet_and_network() {
    // A drawdown call signs nothing per request, so it must NOT require
    // --payment-asset or --max-amount; the pay network defaults to --network.
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

    // Only --network + the key: no asset, no max-amount, no payment-network.
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

/// Sequenced /base-sepolia responder: the FIRST drawdown call 401s with
/// token_expired; the SECOND (after a transparent re-auth) returns the result.
/// `status` is the HTTP code of the expired-token response (the gateway uses
/// 401 or 403).
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
    // Two /auth calls: the initial auth + the re-auth after the expired token.
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
    // The gateway can surface an expired token as a 401.
    assert_drawdown_reauths_on(401).await;
}

#[tokio::test]
async fn x402_drawdown_reauths_once_on_token_expired_403() {
    // ...or as a 403 — both must trigger the transparent re-auth + retry.
    assert_drawdown_reauths_on(403).await;
}

#[tokio::test]
async fn x402_drawdown_rejects_key_and_params_both_from_stdin() {
    // Only one stdin: reading the key from it would silently drain the params.
    // The lane must refuse up front with zero gateway I/O.
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
    // 402 on the drawdown call = no credits; must NOT sign or resend (single
    // attempt), and must surface an actionable "buy-credits" error at exit 2
    // (the gateway refused and nothing settled).
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
    // Refused, nothing settled → exit 2, with a message pointing at the fix.
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("buy-credits"),
        "stderr should point at buy-credits, got: {}",
        out.stderr
    );
}

// ── qn rpc mpp (payment channel session) ─────────────────────────────────────
//
// The MPP session lane opens an escrow channel (an on-chain Tempo tx, signed
// offline against the mock), then pays with cumulative vouchers. The mock
// gateway serves a tempo/session 402 challenge on probes and 2xx on credential
// POSTs; channel status is a GET under /session/:network/channels/:id.

use base64::Engine as _;

// A base64url tempo/session request body (currency, recipient, amount, chainId).
fn session_request_b64() -> String {
    let json = json!({
        "amount": "500",
        "currency": "0x20c0000000000000000000000000000000000000",
        "recipient": "0xfd24114c3981aba78ae2441991b1bdb89329c556",
        "methodDetails": { "chainId": 42431 }
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap())
}

fn session_www_authenticate() -> String {
    format!(
        "Payment id=\"c1\", realm=\"mpp.quicknode.com\", method=\"tempo\", intent=\"session\", description=\"d\", expires=\"2099-01-01T00:00:00Z\", request=\"{}\"",
        session_request_b64()
    )
}

// Mount the session endpoint: a 402 session challenge on an unauthorized POST
// (the probe), and 200 on any POST carrying an Authorization: Payment header
// (credential submissions: open/topUp/voucher/close).
async fn mount_session(server: &MockServer, network: &str) {
    let path_str = format!("/session/{network}");
    Mock::given(method("POST"))
        .and(path(path_str.clone()))
        .and(wiremock::matchers::header_exists("authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": "0xok"
        })))
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

fn mpp_args<'a>(cfg: &'a str, key_path: &'a str, verb: &'a str) -> Vec<&'a str> {
    vec![
        "--config-file",
        cfg,
        "rpc",
        "mpp",
        verb,
        "--network",
        "tempo-testnet",
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
}

#[tokio::test]
async fn mpp_open_without_yes_is_needs_confirmation_and_settles_nothing() {
    let server = MockServer::start().await;
    // Gate is checked before any network I/O: nothing reaches the gateway.
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
async fn mpp_session_call_without_open_channel_points_at_open() {
    let server = MockServer::start().await;
    // No channel cached; the call must refuse before any gateway I/O.
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
