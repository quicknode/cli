//! Re-export of the x402 lifecycle so `qn micropayments x402` and
//! `qn rpc x402` share one runner.

pub use crate::commands::rpc::x402::{run, Args};
