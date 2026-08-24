//! Shared funding noun for x402 credits and MPP channels.
//!
//! `qn rpc x402` and `qn rpc mpp` stay first-class and call the same runners.

pub mod mpp;
pub mod x402;

use clap::{Args as ClapArgs, Subcommand};

use crate::context::GlobalArgs;
use crate::errors::CliError;

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
#[command(after_help = "Examples:\n  \
    qn micropayments x402 drip --payment-wallet payer --payment-network base-sepolia\n  \
    qn micropayments x402 buy-credits --network base-sepolia --yes \\\n      \
    --payment-wallet payer --payment-network base-sepolia \\\n      \
    --payment-asset USDC --max-amount 10000000\n  \
    qn micropayments mpp open --deposit 1000000 --yes \\\n      \
    --payment-wallet payer --payment-network tempo-testnet \\\n      \
    --payment-asset pathUSD --max-amount 1000000\n\n\
    `qn pay` is an alias. `qn rpc x402` and `qn rpc mpp` call the same runners.")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: MicropaymentsCmd,
}

#[derive(Debug, Subcommand)]
pub enum MicropaymentsCmd {
    /// Manage x402 credit drawdown: buy prepaid credits, check the balance, or
    /// drip testnet funds. Pair with `qn sql query --x402-drawdown` or
    /// `qn rpc call --x402-drawdown`.
    X402(x402::Args),

    /// Manage an MPP payment channel: open, top-up, close, or check status.
    /// Pair with `qn sql query --mpp-session` or `qn rpc call --mpp-session`.
    Mpp(mpp::Args),
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        MicropaymentsCmd::X402(a) => x402::run(a, global).await,
        MicropaymentsCmd::Mpp(a) => mpp::run(a, global).await,
    }
}
