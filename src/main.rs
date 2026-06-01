use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use qn::cli::Cli;
use qn::errors::{exit_code_for, render};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let verbose = cli.verbose;
    match cli.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{}", render(&e, verbose));
            ExitCode::from(exit_code_for(&e) as u8)
        }
    }
}
