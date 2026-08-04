//! Subcommand implementations.
//!
//! One module per top-level noun. Each exposes `Args` (the clap-derived
//! argument struct) and `run(args, ctx) -> Result<(), CliError>`.

pub mod agent;
pub mod auth;
pub mod billing;
pub mod chain;
pub mod endpoint;
pub mod kv;
pub mod metrics;
pub mod rpc;
pub mod sql;
pub mod stream;
pub mod team;
pub mod tooling_access;
pub mod usage;
pub mod wallet;
pub mod webhook;
