//! `qn metrics …` — account and endpoint metric series.

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::admin::{GetAccountMetricsRequest, GetEndpointMetricsRequest};
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::Render;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: MetricsCmd,
}

#[derive(Debug, Subcommand)]
pub enum MetricsCmd {
    /// Account-level metric series (credits/calls/latency).
    Account(AccountArgs),
    /// Endpoint-level metric series.
    Endpoint(EndpointArgs),
}

#[derive(Debug, ClapArgs)]
pub struct AccountArgs {
    /// Period.
    #[arg(long, value_parser = ["hour", "day", "week", "month"])]
    pub period: String,
    /// Metric name.
    #[arg(long)]
    pub metric: String,
    /// Percentile (for latency metrics).
    #[arg(long)]
    pub percentile: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct EndpointArgs {
    /// Endpoint id.
    pub id: String,
    /// Period.
    #[arg(long, value_parser = ["hour", "day", "week", "month"])]
    pub period: String,
    /// Metric name.
    #[arg(long)]
    pub metric: String,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        MetricsCmd::Account(a) => account(a, ctx).await,
        MetricsCmd::Endpoint(a) => endpoint(a, ctx).await,
    }
}

async fn account(a: AccountArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = GetAccountMetricsRequest {
        period: a.period,
        metric: a.metric,
        percentile: a.percentile,
    };
    let resp = ctx.sdk.admin.get_account_metrics(&req).await?;
    crate::output::emit(&ctx.out, &AccountMetricsView(resp))
}

async fn endpoint(a: EndpointArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = GetEndpointMetricsRequest {
        period: a.period,
        metric: a.metric,
    };
    let resp = ctx.sdk.admin.get_endpoint_metrics(&a.id, &req).await?;
    crate::output::emit(&ctx.out, &EndpointMetricsView(resp))
}

#[derive(Serialize)]
struct AccountMetricsView(quicknode_sdk::admin::GetAccountMetricsResponse);

impl Render for AccountMetricsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        for m in &self.0.data {
            super::endpoint::render::metric_series(w, m, ctx)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct EndpointMetricsView(quicknode_sdk::admin::GetEndpointMetricsResponse);

impl Render for EndpointMetricsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        for m in &self.0.data {
            super::endpoint::render::metric_series(w, m, ctx)?;
        }
        Ok(())
    }
}
