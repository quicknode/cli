//! Integration tests for `qn sql …`.

mod common;

use common::{run_qn, run_qn_no_key};
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const EVM_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const PATH_USD: &str = "0x20c0000000000000000000000000000000000000";

fn key_file() -> (tempfile::NamedTempFile, String) {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(EVM_KEY.as_bytes()).unwrap();
    f.flush().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    (f, path)
}

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

#[tokio::test]
async fn clusters_and_schema_work_without_an_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sql/rest/v1/clusters"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "hyperliquid-core-mainnet", "display_name": "Hyperliquid (HyperCore)"},
            {"id": "solana-mainnet", "display_name": "Solana"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sql/rest/v1/schema/hyperliquid-core-mainnet"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "chain": "Hyperliquid (HyperCore)",
            "cluster_id": "hyperliquid-core-mainnet",
            "tables": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v0/account/info"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let clusters = run_qn_no_key(&server.uri(), &["--config-file", &cfg, "sql", "clusters"]).await;
    assert_eq!(clusters.exit_code, 0, "stderr={}", clusters.stderr);
    let schema = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "sql",
            "schema",
            "hyperliquid-core-mainnet",
        ],
    )
    .await;
    assert_eq!(schema.exit_code, 0, "stderr={}", schema.stderr);
}

#[tokio::test]
async fn query_without_key_or_payment_flag_names_both_next_steps() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .expect(0)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "sql",
            "query",
            "SELECT 1",
            "--cluster-id",
            "hyperliquid-core-mainnet",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("--x402-drawdown") && out.stderr.contains("--mpp-session"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn drawdown_query_uses_bearer_and_skips_control_plane() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "jwt-test",
            "expiresAt": "2099-01-01T00:00:00Z",
            "accountId": "eip155:84532:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .and(header("authorization", "Bearer jwt-test"))
        .and(|req: &Request| !req.headers.contains_key("x-api-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(query_body()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v0/account/info"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(path("/v0/tooling-access"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "sql",
            "query",
            "SELECT 1",
            "--cluster-id",
            "hyperliquid-core-mainnet",
            "--x402-drawdown",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "base-sepolia",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn drawdown_query_requires_payment_points_at_buy_credits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "jwt-test",
            "expiresAt": "2099-01-01T00:00:00Z",
            "accountId": "eip155:84532:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sql/rest/v1/query"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": "requires_payment"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "sql",
            "query",
            "SELECT 1",
            "--cluster-id",
            "hyperliquid-core-mainnet",
            "--x402-drawdown",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "base-sepolia",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 2, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("qn micropayments x402 buy-credits"),
        "stderr={}",
        out.stderr
    );
}

fn write_channel(dir: &std::path::Path) {
    let text = r#"
[channels]
"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266:eip155:42431:0x20c0000000000000000000000000000000000000" = { channel_id = "0x1111111111111111111111111111111111111111111111111111111111111111", token = "0x20c0000000000000000000000000000000000000", payee = "0xfd24114c3981aba78ae2441991b1bdb89329c556", salt = "0x2222222222222222222222222222222222222222222222222222222222222222", authorized_signer = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266", escrow_contract = "0x33b901018174DDabE4841042ab76ba85D4e24f25", deposit = "1000000", cumulative_spent = "10", per_call = "10", chain_id = 42431 }
"#;
    std::fs::write(dir.join("channels.toml"), text).unwrap();
}

fn sql_session_offer(amount: &str) -> String {
    use base64::Engine;
    let request = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "amount": amount,
            "currency": PATH_USD,
            "recipient": "0xfd24114c3981aba78ae2441991b1bdb89329c556",
            "methodDetails": {
                "chainId": 42431,
                "escrowContract": "0x33b901018174DDabE4841042ab76ba85D4e24f25"
            }
        }))
        .unwrap(),
    );
    format!(
        "Payment id=\"sql1\", realm=\"mpp.quicknode.com\", method=\"tempo\", \
         intent=\"session\", description=\"d\", expires=\"2099-01-01T00:00:00Z\", \
         request=\"{request}\""
    )
}

fn receipt_header(accepted: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "acceptedCumulative": accepted,
            "spent": accepted,
            "status": "success",
            "intent": "session",
            "method": "tempo"
        }))
        .unwrap(),
    )
}

#[tokio::test]
async fn mpp_session_query_uses_challenge_amount_and_advances_cache() {
    struct SqlSeq {
        calls: AtomicUsize,
    }
    impl Respond for SqlSeq {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 && !req.headers.contains_key("authorization") {
                return ResponseTemplate::new(402)
                    .insert_header("www-authenticate", sql_session_offer("100"));
            }
            use base64::Engine;
            let auth = req
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let b64 = auth.strip_prefix("Payment ").unwrap_or(auth);
            let cred: serde_json::Value = serde_json::from_slice(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(b64.trim_end_matches('='))
                    .unwrap(),
            )
            .unwrap();
            let cumulative = cred["payload"]["cumulativeAmount"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            if cumulative == 20 {
                return ResponseTemplate::new(402).set_body_json(json!({
                    "title": "Insufficient Balance",
                    "detail": "Insufficient balance: requested 100, available 10."
                }));
            }
            assert_eq!(cumulative, 110);
            ResponseTemplate::new(200)
                .insert_header("payment-receipt", receipt_header("110").as_str())
                .set_body_json(query_body())
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/session/sql/rest/v1/query"))
        .respond_with(SqlSeq {
            calls: AtomicUsize::new(0),
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/v0/account/info"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    write_channel(dir.path());
    let cfg = dir.path().join("config.toml").to_str().unwrap().to_string();
    let (_guard, key_path) = key_file();
    let out = run_qn_no_key(
        &server.uri(),
        &[
            "--config-file",
            &cfg,
            "sql",
            "query",
            "SELECT 1",
            "--cluster-id",
            "hyperliquid-core-mainnet",
            "--mpp-session",
            "--payment-key-file",
            &key_path,
            "--payment-network",
            "tempo-testnet",
            "--payment-asset",
            "pathUSD",
            "--max-amount",
            "1000000",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let cache = std::fs::read_to_string(dir.path().join("channels.toml")).unwrap();
    assert!(
        cache.contains("cumulative_spent = \"110\""),
        "cache must advance to acceptedCumulative, got: {cache}"
    );
}
