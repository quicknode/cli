//! `qn usage …` — account usage.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::GetUsageRequest;
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};
use crate::retry::retrying;
use crate::time_arg;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: UsageCmd,
}

#[derive(Debug, Subcommand)]
pub enum UsageCmd {
    /// Aggregate usage summary.
    Summary(Range),
    /// Per-endpoint usage breakdown.
    ByEndpoint(Range),
    /// Per-RPC-method usage breakdown.
    ByMethod(Range),
    /// Per-chain usage breakdown.
    ByChain(Range),
    /// Per-tag usage breakdown.
    ByTag(Range),
}

#[derive(Debug, ClapArgs)]
pub struct Range {
    /// Start time (RFC-3339, relative like `7d`, or `now`). Omit for account-to-date.
    #[arg(long)]
    pub from: Option<String>,
    /// End time. Omit for now.
    #[arg(long)]
    pub to: Option<String>,
}

impl Range {
    fn into_request(self) -> Result<GetUsageRequest, CliError> {
        Ok(GetUsageRequest {
            start_time: self.from.as_deref().map(time_arg::parse_unix).transpose()?,
            end_time: self.to.as_deref().map(time_arg::parse_unix).transpose()?,
        })
    }
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        UsageCmd::Summary(r) => summary(r, ctx).await,
        UsageCmd::ByEndpoint(r) => by_endpoint(r, ctx).await,
        UsageCmd::ByMethod(r) => by_method(r, ctx).await,
        UsageCmd::ByChain(r) => by_chain(r, ctx).await,
        UsageCmd::ByTag(r) => by_tag(r, ctx).await,
    }
}

async fn summary(r: Range, ctx: Ctx) -> Result<(), CliError> {
    let req = r.into_request()?;
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_usage(&req)).await?;
    crate::output::emit(&ctx.out, &UsageSummaryView(resp))
}

async fn by_endpoint(r: Range, ctx: Ctx) -> Result<(), CliError> {
    let req = r.into_request()?;
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_usage_by_endpoint(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &UsageByEndpointView(resp))
}

async fn by_method(r: Range, ctx: Ctx) -> Result<(), CliError> {
    let req = r.into_request()?;
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_usage_by_method(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &UsageByMethodView(resp))
}

async fn by_chain(r: Range, ctx: Ctx) -> Result<(), CliError> {
    let req = r.into_request()?;
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_usage_by_chain(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &UsageByChainView(resp))
}

async fn by_tag(r: Range, ctx: Ctx) -> Result<(), CliError> {
    let req = r.into_request()?;
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_usage_by_tag(&req)).await?;
    crate::output::emit(&ctx.out, &UsageByTagView(resp))
}

// ----- renderers ----- //

#[derive(Serialize)]
struct UsageSummaryView(quicknode_sdk::admin::GetUsageResponse);

impl Render for UsageSummaryView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no usage data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![
            Cell::new("credits_used"),
            Cell::new(data.credits_used),
        ]);
        t.add_row(vec![
            Cell::new("credits_remaining"),
            opt_cell(&data.credits_remaining),
        ]);
        t.add_row(vec![Cell::new("limit"), opt_cell(&data.limit)]);
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct UsageByEndpointView(quicknode_sdk::admin::GetUsageByEndpointResponse);

impl Render for UsageByEndpointView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no usage data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ENDPOINT", "CHAIN", "NETWORK", "CREDITS"]);
        for e in &data.endpoints {
            t.add_row(vec![
                Cell::new(&e.name),
                opt_cell(&e.chain),
                opt_cell(&e.network),
                Cell::new(e.credits_used),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct UsageByMethodView(quicknode_sdk::admin::GetUsageByMethodResponse);

impl Render for UsageByMethodView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no usage data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["METHOD", "CREDITS", "ARCHIVE"]);
        for m in &data.methods {
            t.add_row(vec![
                Cell::new(&m.method_name),
                Cell::new(m.credits_used),
                opt_cell(&m.archive),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct UsageByChainView(quicknode_sdk::admin::GetUsageByChainResponse);

impl Render for UsageByChainView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no usage data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["CHAIN", "CREDITS"]);
        for c in &data.chains {
            t.add_row(vec![Cell::new(&c.name), Cell::new(c.credits_used)]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct UsageByTagView(quicknode_sdk::admin::GetUsageByTagResponse);

impl Render for UsageByTagView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no usage data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["TAG_ID", "LABEL", "CREDITS"]);
        for tg in &data.tags {
            t.add_row(vec![
                opt_cell(&tg.tag_id),
                Cell::new(&tg.label),
                Cell::new(tg.credits_used),
            ]);
        }
        write_table(w, &t)
    }
}
