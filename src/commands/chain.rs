//! `qn chain …` — list supported blockchains.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, Render};
use crate::retry::retrying;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: ChainCmd,
}

#[derive(Debug, Subcommand)]
pub enum ChainCmd {
    /// List supported chains and their networks.
    #[command(visible_alias = "ls")]
    List,
    /// Show per-method API credit costs for a chain.
    Credits {
        /// Chain slug (e.g. `ethereum`; run `qn chain list` to see all).
        #[arg(value_name = "CHAIN")]
        chain: String,
    },
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        ChainCmd::List => list(ctx).await,
        ChainCmd::Credits { chain } => credits(chain, ctx).await,
    }
}

async fn list(ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.list_chains()).await?;
    crate::output::emit(&ctx.out, &ChainsView(resp))
}

async fn credits(chain: String, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_api_credits(&chain)).await?;
    crate::output::emit(&ctx.out, &ApiCreditsView(resp))
}

#[derive(Serialize)]
struct ChainsView(quicknode_sdk::admin::ListChainsResponse);

impl Render for ChainsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["CHAIN", "NETWORKS"]);
        for c in &self.0.data {
            let nets = c
                .networks
                .iter()
                .map(|n| n.slug.clone())
                .collect::<Vec<_>>()
                .join(", ");
            t.add_row(vec![Cell::new(&c.slug), Cell::new(nets)]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct ApiCreditsView(quicknode_sdk::admin::GetApiCreditsResponse);

impl Render for ApiCreditsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let rows = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no credits)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["METHOD", "CREDITS"]);
        for c in rows {
            t.add_row(vec![Cell::new(&c.method), Cell::new(c.credits)]);
        }
        write_table(w, &t)
    }
}
