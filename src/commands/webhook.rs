//! `qn webhook …` — stub. Will be filled in by a later stage.

use clap::Args as ClapArgs;

use crate::context::Ctx;
use crate::errors::CliError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(hide = true)]
    _rest: Vec<String>,
}

pub async fn run(_args: Args, _ctx: Ctx) -> Result<(), CliError> {
    Err(CliError::Arg(
        "qn webhook is not yet implemented in this build".to_string(),
    ))
}
