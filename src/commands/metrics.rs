//! `qn metrics …` — account and endpoint metric series, plus the windowed
//! traffic and error summaries. `qn endpoint errors` dispatches here too so
//! the account and endpoint error summaries share one renderer.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    ErrorsSummaryData, ErrorsSummaryRow, GetAccountMetricsRequest, GetEndpointErrorsSummaryRequest,
    GetEndpointMetricsRequest, GetErrorsSummaryRequest, GetTrafficSummaryRequest,
    TrafficSummaryRow,
};
use serde::Serialize;

use super::summary::{breakdown_table, dim, non_empty, write_summary, SummaryPage, WindowArgs};
use super::usage::RpcFilterArgs;
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{opt_cell, OutputCtx, Render};
use crate::retry::retrying;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn metrics account --period day --metric credits_over_time\n  \
    qn metrics endpoint ep-1 --period day --metric response_time_over_time --percentile 95\n  \
    qn metrics traffic --from 24h --group-by endpoint\n  \
    qn metrics errors --from 1h --kind rpc --group-by code")]
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
    /// Traffic totals over a time window, optionally grouped by method or endpoint.
    Traffic(TrafficArgs),
    /// Account-wide error totals over a time window, optionally grouped and filtered.
    Errors(ErrorsArgs),
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn metrics traffic --from 24h\n  \
    qn metrics traffic --from 7d --group-by method,endpoint --limit 50\n  \
    qn metrics traffic --from 1h --chain ethereum --method eth_call\n  \
    qn metrics traffic --cursor eyJvZmZzZXQiOjEwMH0")]
pub struct TrafficArgs {
    #[command(flatten)]
    pub window: WindowArgs,
    /// Group rows by these dimensions (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',', value_parser = ["method", "endpoint"])]
    pub group_by: Vec<String>,
    #[command(flatten)]
    pub filters: RpcFilterArgs,
}

/// Filters shared by `metrics errors` and `endpoint errors`.
#[derive(Debug, ClapArgs)]
pub struct ErrorFilterArgs {
    /// Error kind. Method filters and grouping need `rpc`; --status-code needs `http`.
    #[arg(long, value_parser = ["rpc", "http"])]
    pub kind: Option<String>,
    /// Only these networks (repeatable).
    #[arg(long = "network")]
    pub networks: Vec<String>,
    /// Only these RPC methods (repeatable; rpc kind).
    #[arg(long = "method")]
    pub methods: Vec<String>,
    /// Only these RPC error codes (repeatable; rpc kind).
    // RPC error codes are negative (`-32000`); let clap read them as values.
    #[arg(long = "code", allow_hyphen_values = true)]
    pub codes: Vec<String>,
    /// Only these HTTP status codes (repeatable; http kind).
    #[arg(long = "status-code")]
    pub status_codes: Vec<String>,
}

impl ErrorFilterArgs {
    fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.networks.is_empty()
            && self.methods.is_empty()
            && self.codes.is_empty()
            && self.status_codes.is_empty()
    }
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn metrics errors --from 1h\n  \
    qn metrics errors --from 24h --kind rpc --group-by code,method\n  \
    qn metrics errors --from 24h --kind http --status-code 429 --endpoint 123\n  \
    qn metrics errors --cursor eyJvZmZzZXQiOjEwMH0")]
pub struct ErrorsArgs {
    #[command(flatten)]
    pub window: WindowArgs,
    /// Group rows by these dimensions (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',', value_parser = ["code", "endpoint", "network", "method"])]
    pub group_by: Vec<String>,
    /// Only these numeric endpoint ids (repeatable).
    #[arg(long = "endpoint", value_name = "NUMERIC_ID")]
    pub endpoints: Vec<i64>,
    #[command(flatten)]
    pub filters: ErrorFilterArgs,
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn endpoint errors ep-1 --from 1h\n  \
    qn endpoint errors ep-1 --from 24h --kind rpc --group-by code\n  \
    qn endpoint errors ep-1 --from 24h --kind http --status-code 500\n  \
    qn endpoint errors ep-1 --cursor eyJvZmZzZXQiOjEwMH0")]
pub struct EndpointErrorsArgs {
    /// Endpoint id.
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    #[command(flatten)]
    pub window: WindowArgs,
    /// Group rows by these dimensions (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',', value_parser = ["code", "network", "method"])]
    pub group_by: Vec<String>,
    #[command(flatten)]
    pub filters: ErrorFilterArgs,
}

#[derive(Debug, ClapArgs)]
pub struct AccountArgs {
    /// Period.
    #[arg(long, value_parser = ["hour", "day", "week", "month"])]
    pub period: String,
    /// Metric name (e.g. `method_calls_over_time`, `metered_endpoint_data_over_time`,
    /// `metered_endpoint_credits_over_time`, `credits_over_time`).
    #[arg(long)]
    pub metric: String,
    /// Percentile (for latency metrics).
    #[arg(long)]
    pub percentile: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct EndpointArgs {
    /// Endpoint id.
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    /// Period.
    #[arg(long, value_parser = ["hour", "day", "week", "month"])]
    pub period: String,
    /// Metric name (e.g. `method_calls_over_time`, `metered_endpoint_data_over_time`,
    /// `metered_endpoint_credits_over_time`, `credits_over_time`, `response_status_breakdown`).
    #[arg(long)]
    pub metric: String,
    /// Percentile (1-100); required for response-time metrics.
    #[arg(long, value_parser = clap::value_parser!(i32).range(1..=100))]
    pub percentile: Option<i32>,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        MetricsCmd::Account(a) => account(a, ctx).await,
        MetricsCmd::Endpoint(a) => endpoint(a, ctx).await,
        MetricsCmd::Traffic(a) => traffic(a, ctx).await,
        MetricsCmd::Errors(a) => errors(a, ctx).await,
    }
}

async fn traffic(a: TrafficArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a
        .window
        .resolve(!a.group_by.is_empty() || !a.filters.is_empty())?;
    let f = a.filters;
    let req = GetTrafficSummaryRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        cursor: w.cursor,
        limit: w.limit,
        group_by: non_empty(a.group_by),
        endpoint_names: non_empty(f.endpoint_names),
        chains: non_empty(f.chains),
        networks: non_empty(f.networks),
        methods: non_empty(f.methods),
        tag_ids: non_empty(f.tag_ids),
        addon_ids: non_empty(f.addon_ids),
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_traffic_summary(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &TrafficSummaryView(resp))
}

async fn errors(a: ErrorsArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a
        .window
        .resolve(!a.group_by.is_empty() || !a.endpoints.is_empty() || !a.filters.is_empty())?;
    let f = a.filters;
    let req = GetErrorsSummaryRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        cursor: w.cursor,
        limit: w.limit,
        group_by: non_empty(a.group_by),
        endpoints: non_empty(a.endpoints),
        networks: non_empty(f.networks),
        methods: non_empty(f.methods),
        codes: non_empty(f.codes),
        status_codes: non_empty(f.status_codes),
        kind: f.kind,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_errors_summary(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &ErrorsSummaryView(resp))
}

/// `qn endpoint errors <ID>`: the per-endpoint error summary route.
pub async fn endpoint_errors(a: EndpointErrorsArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a
        .window
        .resolve(!a.group_by.is_empty() || !a.filters.is_empty())?;
    let f = a.filters;
    let req = GetEndpointErrorsSummaryRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        cursor: w.cursor,
        limit: w.limit,
        group_by: non_empty(a.group_by),
        networks: non_empty(f.networks),
        methods: non_empty(f.methods),
        codes: non_empty(f.codes),
        status_codes: non_empty(f.status_codes),
        kind: f.kind,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_endpoint_errors_summary(&a.id, &req)
    })
    .await?;
    crate::output::emit(&ctx.out, &EndpointErrorsSummaryView(resp))
}

async fn account(a: AccountArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = GetAccountMetricsRequest {
        period: a.period,
        metric: a.metric,
        percentile: a.percentile,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_account_metrics(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &AccountMetricsView(resp))
}

async fn endpoint(a: EndpointArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = GetEndpointMetricsRequest {
        period: a.period,
        metric: a.metric,
        percentile: a.percentile,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_endpoint_metrics(&a.id, &req)
    })
    .await?;
    crate::output::emit(&ctx.out, &EndpointMetricsView(resp))
}

#[derive(Serialize)]
struct TrafficSummaryView(quicknode_sdk::admin::GetTrafficSummaryResponse);

impl Render for TrafficSummaryView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let d = &self.0.data;
        let t = &d.totals;
        let breakdown = d.breakdown.as_deref().map(|rows| {
            breakdown_table(
                rows,
                &[
                    ("METHOD", |r: &TrafficSummaryRow| dim(&r.method)),
                    ("ENDPOINT", |r: &TrafficSummaryRow| dim(&r.endpoint)),
                ],
                &[
                    "SUCCESSFUL_REQUESTS",
                    "BYTES",
                    "REQUEST_CREDITS",
                    "BYTES_CREDITS",
                    "CREDITS",
                    "ACTIVE_METHODS",
                ],
                |r| {
                    vec![
                        opt_cell(&r.successful_requests),
                        Cell::new(r.bytes),
                        Cell::new(r.request_credits),
                        Cell::new(r.bytes_credits),
                        Cell::new(r.credits),
                        Cell::new(r.active_methods),
                    ]
                },
            )
        });
        write_summary(
            w,
            ctx,
            SummaryPage {
                from: &d.from_time,
                to: &d.to_time,
                totals: vec![
                    ("successful_requests", Cell::new(t.successful_requests)),
                    ("bytes", Cell::new(t.bytes)),
                    ("request_credits", Cell::new(t.request_credits)),
                    ("bytes_credits", Cell::new(t.bytes_credits)),
                    ("credits", Cell::new(t.credits)),
                    ("active_methods", Cell::new(t.active_methods)),
                ],
                breakdown,
                next_cursor: d.next_cursor.as_deref(),
            },
        )
    }
}

/// Shared by the account and endpoint error summaries: same payload, two routes.
fn render_errors_summary(
    w: &mut dyn std::io::Write,
    ctx: &OutputCtx,
    d: &ErrorsSummaryData,
) -> std::io::Result<()> {
    let breakdown = d.breakdown.as_deref().map(|rows| {
        breakdown_table(
            rows,
            &[
                ("CODE", |r: &ErrorsSummaryRow| dim(&r.code)),
                ("ENDPOINT", |r: &ErrorsSummaryRow| dim(&r.endpoint)),
                ("NETWORK", |r: &ErrorsSummaryRow| dim(&r.network)),
                ("METHOD", |r: &ErrorsSummaryRow| dim(&r.method)),
            ],
            &["KIND", "COUNT"],
            |r| vec![Cell::new(&r.kind), Cell::new(r.count)],
        )
    });
    write_summary(
        w,
        ctx,
        SummaryPage {
            from: &d.from_time,
            to: &d.to_time,
            totals: vec![
                ("rpc_errors", Cell::new(d.totals.rpc_errors)),
                ("http_errors", Cell::new(d.totals.http_errors)),
            ],
            breakdown,
            next_cursor: d.next_cursor.as_deref(),
        },
    )
}

#[derive(Serialize)]
struct ErrorsSummaryView(quicknode_sdk::admin::GetErrorsSummaryResponse);

impl Render for ErrorsSummaryView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        render_errors_summary(w, ctx, &self.0.data)
    }
}

#[derive(Serialize)]
struct EndpointErrorsSummaryView(quicknode_sdk::admin::GetEndpointErrorsSummaryResponse);

impl Render for EndpointErrorsSummaryView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        render_errors_summary(w, ctx, &self.0.data)
    }
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
