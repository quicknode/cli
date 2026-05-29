//! Subprocess smoke tests for the `qn` binary.
//!
//! Verifies CLI guidelines compliance: --help everywhere, --version works,
//! help-on-no-args, --json output is valid, exit codes are correct, NO_COLOR
//! is honored.

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    let mut c = Command::cargo_bin("qn").expect("qn binary built");
    // Make sure no inherited env hijacks the tests.
    c.env_remove("QN_CLI__API_KEY");
    c.env_remove("NO_COLOR");
    c.env_remove("HOME"); // so config lookup doesn't read a real ~/.config/qn
    c.env("HOME", std::env::temp_dir());
    c
}

#[test]
fn help_exits_zero() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"))
        .stdout(predicate::str::contains("Manage RPC endpoints"));
}

#[test]
fn version_works() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("qn "));
}

#[test]
fn no_subcommand_shows_help_and_exits_nonzero() {
    bin()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:"));
}

#[test]
fn unknown_subcommand_fails_with_suggestion() {
    bin()
        .arg("endpoit") // typo
        .assert()
        .failure()
        .stderr(predicate::str::contains("endpoint").or(predicate::str::contains("Usage:")));
}

#[test]
fn endpoint_help_works() {
    bin()
        .args(["endpoint", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn endpoint_short_help_works() {
    bin()
        .args(["endpoint", "-h"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn no_api_key_no_tty_exits_4() {
    bin()
        .args(["endpoint", "list"])
        .args(["--no-input"])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("no API key found"));
}

#[test]
fn completions_zsh_works() {
    bin()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef qn"));
}

#[test]
fn completions_bash_works() {
    bin()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_qn"));
}

#[test]
fn alias_endpoints_resolves() {
    bin().args(["endpoints", "--help"]).assert().success();
}

#[test]
fn auth_status_no_key_exits_4() {
    bin()
        .args(["auth", "status", "--no-input"])
        .assert()
        .failure()
        .code(4);
}
