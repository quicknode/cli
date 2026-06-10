use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use qn::cli::Cli;
use qn::errors::{exit_code_for, render};

#[tokio::main]
async fn main() -> ExitCode {
    // Map clap usage errors to exit 1 — the same bucket as runtime argument
    // errors — so exit 2 always and only means "the API returned an error".
    // (clap's own Error::exit would use 2 for both.)
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // --help/--version print to stdout and are not errors.
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    let verbose = cli.verbose;
    match cli.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{}", render(&e, verbose));
            ExitCode::from(exit_code_for(&e) as u8)
        }
    }
}
