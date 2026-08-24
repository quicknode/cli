//! Re-export of the MPP lifecycle so `qn micropayments mpp` and
//! `qn rpc mpp` share one runner.

pub use crate::commands::rpc::mpp::{run, Args};
