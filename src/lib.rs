//! Library entry point for the `qn` CLI.
//!
//! This module exists so integration tests can call into the CLI in-process.
//! See `tests/common/mod.rs` for the test harness.

pub mod cli;
pub mod commands;
pub mod config;
pub mod confirm;
pub mod context;
pub mod errors;
pub mod output;
pub mod time_arg;

pub use cli::Cli;
pub use errors::CliError;
