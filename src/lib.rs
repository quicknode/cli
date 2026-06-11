//! Library entry point for the `qn` CLI.
//!
//! This module exists so integration tests can call into the CLI in-process.
//! See `tests/common/mod.rs` for the test harness.

pub mod cli;
pub mod context;
pub mod errors;
pub mod output;

pub(crate) mod commands;
pub(crate) mod config;
pub(crate) mod confirm;
pub(crate) mod retry;
pub(crate) mod time_arg;

pub use cli::Cli;
pub use errors::CliError;
