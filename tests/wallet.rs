//! Integration tests for the local payment-wallet store.

mod common;

use common::run_qn;

// Wallet commands never make a request; the harness still requires a base URL.
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
            "wallet",
            "generate",
            "--vm",
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

    let raw = std::fs::read_to_string(&key).unwrap();
    assert!(raw.trim().starts_with("0x"), "key not 0x-prefixed: {raw}");

    let meta_text = std::fs::read_to_string(&meta).unwrap();
    assert!(meta_text.contains("vm = \"evm\""), "meta: {meta_text}");
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
            "wallet",
            "generate",
            "--vm",
            "svm",
            "--name",
            "sol",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);

    let raw = std::fs::read_to_string(wallets_dir(dir.path()).join("sol")).unwrap();
    assert!(!raw.trim().starts_with("0x"));
    assert!(
        raw.trim().len() > 64,
        "svm key too short: {}",
        raw.trim().len()
    );

    let meta = std::fs::read_to_string(wallets_dir(dir.path()).join("sol.toml")).unwrap();
    assert!(meta.contains("vm = \"svm\""), "meta: {meta}");
}

#[tokio::test]
async fn generate_refuses_overwrite_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let args = &[
        "--config-file",
        &cfg,
        "wallet",
        "generate",
        "--vm",
        "evm",
        "--name",
        "dup",
    ];
    assert_eq!(run_qn(BASE, args).await.exit_code, 0);

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
            "wallet",
            "generate",
            "--vm",
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
                "wallet",
                "generate",
                "--vm",
                "evm",
                "--name",
                "keep",
            ],
        )
        .await
        .exit_code,
        0
    );

    let out = run_qn(BASE, &["--config-file", &cfg, "wallet", "rm", "keep"]).await;
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
                "wallet",
                "generate",
                "--vm",
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
        &["--config-file", &cfg, "wallet", "rm", "gone", "--yes"],
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
        &["--config-file", &cfg, "wallet", "rm", "nope", "--yes"],
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
    let out = run_qn(BASE, &["--config-file", &cfg, "wallet", "show", "ghost"]).await;
    assert_eq!(out.exit_code, 1, "stderr={}", out.stderr);
    assert!(
        out.stderr.contains("no wallet named"),
        "stderr={}",
        out.stderr
    );
}

// Use the real binary to verify stdout/stderr separation.
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
            "wallet",
            "generate",
            "--vm",
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

    assert!(stdout.trim().starts_with("0x"), "stdout={stdout}");

    let key_path = wallets_dir(dir.path()).join("payer");
    assert!(
        stderr.contains(&format!("Private key file: {}", key_path.display())),
        "stderr missing labeled key path: {stderr}"
    );
    assert!(
        stderr.contains("stored only on this machine") && stderr.contains("Quicknode does not"),
        "stderr missing custody note: {stderr}"
    );
    let raw = std::fs::read_to_string(&key_path).unwrap();
    assert!(!stdout.contains(raw.trim()) && !stderr.contains(raw.trim()));
}
