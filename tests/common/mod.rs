//! Shared helpers for integration tests.
//!
//! The bulk of CLI integration tests run *in-process* (no subprocess) for
//! speed and reliability. We parse with `Cli::try_parse_from`, point the SDK
//! at a `wiremock` server via `--base-url`, and dispatch with `Cli::run()`.
//! Stdout is NOT captured by the in-process harness — assertions go through
//! exit codes and wiremock request matchers. Tests that need to assert on
//! rendered output live in `tests/cli_smoke.rs` and run via `assert_cmd`.

#![allow(dead_code)]

use clap::Parser;
use qn::cli::Cli;
use qn::errors::{exit_code_for, render_with_argv};

/// Result of dispatching a CLI invocation in-process.
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run the CLI in-process with the given argv (`qn …` prepended automatically).
/// The SDK is pointed at `base_url`, an API key of `"test"` is supplied, and
/// `--no-input` is set so no command can hang on a prompt.
pub async fn run_qn(base_url: &str, extra_args: &[&str]) -> RunOutput {
    // We can't easily redirect stdout/stderr of the parent process from within
    // a tokio test, so we don't try. Tests assert via wiremock matchers (for
    // request shape) and via the exit code; visible-output assertions are in
    // the cli_smoke.rs subprocess tests instead.
    let mut argv: Vec<String> = vec![
        "qn".to_string(),
        "--api-key".to_string(),
        "test".to_string(),
        "--base-url".to_string(),
        base_url.to_string(),
        "--no-input".to_string(),
        "--no-color".to_string(),
    ];
    argv.extend(extra_args.iter().map(|s| s.to_string()));

    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            let exit = if e.use_stderr() { 2 } else { 0 };
            return RunOutput {
                stdout: String::new(),
                stderr: e.to_string(),
                exit_code: exit,
            };
        }
    };

    let verbose = cli.verbose;
    // Use the simulated argv (skip the leading "qn" token) so did-you-mean
    // suggestions can see what the test "user" passed.
    let argv_for_render: Vec<String> = argv.iter().skip(1).cloned().collect();
    match cli.run().await {
        Ok(()) => RunOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        },
        Err(e) => RunOutput {
            stdout: String::new(),
            stderr: render_with_argv(&e, verbose, &argv_for_render),
            exit_code: exit_code_for(&e),
        },
    }
}

/// Build a CLI without dispatching, useful for arg-parse-only tests.
pub fn parse(extra_args: &[&str]) -> Result<Cli, clap::Error> {
    let mut argv: Vec<String> = vec!["qn".to_string()];
    argv.extend(extra_args.iter().map(|s| s.to_string()));
    Cli::try_parse_from(&argv)
}
