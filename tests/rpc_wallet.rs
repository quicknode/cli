//! Integration tests for `qn rpc wallet …` — the local payment-wallet store.
//!
//! These commands are keyless and make no HTTP calls, so there's no wiremock
//! gateway here. The in-process harness doesn't capture stdout, so assertions
//! go through exit codes and on-disk effects (the key file, its 0600 perms,
//! and the metadata sidecar). `--config-file` points the wallet store at a
//! tempdir so nothing touches the real config.

mod common;

use common::run_qn;

// Any URL works — wallet commands never make a request. The harness requires a
// --base-url value.
const BASE: &str = "http://127.0.0.1:1";

fn cfg_in(dir: &std::path::Path) -> String {
    dir.join("config.toml").to_str().unwrap().to_string()
}

fn wallets_dir(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("wallets")
}

#[tokio::test]
async fn generate_writes_key_and_sidecar_at_0600() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let out = run_qn(
        BASE,
        &[
            "--config-file",
            &cfg,
            "rpc",
            "wallet",
            "generate",
            "--chain",
            "evm",
            "--name",
            "payer",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let key = wallets_dir(dir.path()).join("payer");
    let meta = wallets_dir(dir.path()).join("payer.toml");
    assert!(key.exists(), "key file missing");
    assert!(meta.exists(), "metadata sidecar missing");

    // The stored key is a valid 0x-prefixed hex secp256k1 key.
    let raw = std::fs::read_to_string(&key).unwrap();
    assert!(raw.trim().starts_with("0x"), "key not 0x-prefixed: {raw}");

    // The sidecar records the chain + a derived 0x address, never the key.
    let meta_text = std::fs::read_to_string(&meta).unwrap();
    assert!(meta_text.contains("chain = \"evm\""), "meta: {meta_text}");
    assert!(meta_text.contains("0x"), "meta has no address: {meta_text}");
    assert!(!meta_text.contains(raw.trim()), "sidecar leaked the key!");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key file not 0600: {mode:o}");
        let dir_mode = std::fs::metadata(wallets_dir(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            dir_mode & 0o777,
            0o700,
            "wallets dir not 0700: {dir_mode:o}"
        );
    }
}

#[tokio::test]
async fn generate_svm_stores_base58_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let out = run_qn(
        BASE,
        &[
            "--config-file",
            &cfg,
            "rpc",
            "wallet",
            "generate",
            "--chain",
            "svm",
            "--name",
            "sol",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let raw = std::fs::read_to_string(wallets_dir(dir.path()).join("sol")).unwrap();
    // base58 key: not 0x-prefixed hex, and non-trivially long (64-byte base58 is
    // ~88 chars). The SDK's round-trip unit tests cover exact decoding.
    assert!(!raw.trim().starts_with("0x"));
    assert!(
        raw.trim().len() > 64,
        "svm key too short: {}",
        raw.trim().len()
    );

    // The address in the sidecar is the base58 pubkey, not a 0x address.
    let meta = std::fs::read_to_string(wallets_dir(dir.path()).join("sol.toml")).unwrap();
    assert!(meta.contains("chain = \"svm\""), "meta: {meta}");
}

#[tokio::test]
async fn generate_refuses_overwrite_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let args = &[
        "--config-file",
        &cfg,
        "rpc",
        "wallet",
        "generate",
        "--chain",
        "evm",
        "--name",
        "dup",
    ];
    assert_eq!(run_qn(BASE, args).await.exit_code, 0);

    // Second generate without --force must fail and leave the key untouched.
    let key = wallets_dir(dir.path()).join("dup");
    let before = std::fs::read_to_string(&key).unwrap();
    let out = run_qn(BASE, args).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("already exists"),
        "stderr={}",
        out.stderr
    );
    let after = std::fs::read_to_string(&key).unwrap();
    assert_eq!(before, after, "key was overwritten without --force");
}

#[tokio::test]
async fn generate_rejects_unsafe_name() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let out = run_qn(
        BASE,
        &[
            "--config-file",
            &cfg,
            "rpc",
            "wallet",
            "generate",
            "--chain",
            "evm",
            "--name",
            "../escape",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("invalid wallet name"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn rm_without_yes_non_tty_needs_confirmation_and_keeps_files() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    assert_eq!(
        run_qn(
            BASE,
            &[
                "--config-file",
                &cfg,
                "rpc",
                "wallet",
                "generate",
                "--chain",
                "evm",
                "--name",
                "keep",
            ],
        )
        .await
        .exit_code,
        0
    );

    // Non-TTY without --yes: exit 5, and both files remain.
    let out = run_qn(
        BASE,
        &["--config-file", &cfg, "rpc", "wallet", "rm", "keep"],
    )
    .await;
    assert_eq!(out.exit_code, 5, "stderr={}", out.stderr);
    assert!(wallets_dir(dir.path()).join("keep").exists());
    assert!(wallets_dir(dir.path()).join("keep.toml").exists());
}

#[tokio::test]
async fn rm_with_yes_deletes_key_and_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    assert_eq!(
        run_qn(
            BASE,
            &[
                "--config-file",
                &cfg,
                "rpc",
                "wallet",
                "generate",
                "--chain",
                "evm",
                "--name",
                "gone",
            ],
        )
        .await
        .exit_code,
        0
    );

    let out = run_qn(
        BASE,
        &[
            "--config-file",
            &cfg,
            "rpc",
            "wallet",
            "rm",
            "gone",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
    assert!(!wallets_dir(dir.path()).join("gone").exists());
    assert!(!wallets_dir(dir.path()).join("gone.toml").exists());
}

#[tokio::test]
async fn rm_unknown_wallet_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let out = run_qn(
        BASE,
        &[
            "--config-file",
            &cfg,
            "rpc",
            "wallet",
            "rm",
            "nope",
            "--yes",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("no wallet named"),
        "stderr={}",
        out.stderr
    );
}

#[tokio::test]
async fn show_unknown_wallet_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let out = run_qn(
        BASE,
        &["--config-file", &cfg, "rpc", "wallet", "show", "ghost"],
    )
    .await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("no wallet named"),
        "stderr={}",
        out.stderr
    );
}

// Subprocess: the in-process harness can't capture stdout/stderr, so assert the
// generate output split (address on stdout; key path + custody note on stderr)
// via the real binary.
#[tokio::test]
async fn generate_prints_key_path_and_custody_note() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let output = assert_cmd::Command::cargo_bin("qn")
        .unwrap()
        .args([
            "--config-file",
            &cfg,
            "--no-input",
            "--no-color",
            "rpc",
            "wallet",
            "generate",
            "--chain",
            "evm",
            "--name",
            "payer",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    // Address is the only thing on stdout (the pipeable value).
    assert!(stdout.trim().starts_with("0x"), "stdout={stdout}");

    // Key path and custody note go to stderr.
    let key_path = wallets_dir(dir.path()).join("payer");
    assert!(
        stderr.contains(&key_path.display().to_string()),
        "stderr missing key path: {stderr}"
    );
    assert!(
        stderr.contains("stored only on this machine") && stderr.contains("Quicknode does not"),
        "stderr missing custody note: {stderr}"
    );
    // The raw key must never appear on either stream.
    let raw = std::fs::read_to_string(&key_path).unwrap();
    assert!(!stdout.contains(raw.trim()) && !stderr.contains(raw.trim()));
}
