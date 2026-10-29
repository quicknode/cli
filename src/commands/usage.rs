//! `qn usage …` — account usage.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    GetUsageCreditsSummaryRequest, GetUsageRequest, GetUsageRpcSummaryRequest,
    GetUsageStreamsSummaryRequest, GetUsageWebhooksSummaryRequest, UsageRpcSummaryRow,
    UsageStreamsSummaryRow, UsageWebhooksSummaryRow,
};
use serde::Serialize;

use super::summary::{breakdown_table, dim, non_empty, write_summary, SummaryPage, WindowArgs};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{bool_cell, new_table, opt_cell, set_header_bold, write_table, Render};
use crate::retry::retrying;
use crate::time_arg;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn usage summary --from 7d\n  \
    qn usage by-endpoint --from 30d --wide\n  \
    qn usage credits --from 24h\n  \
    qn usage rpc --from 1h --group-by method,endpoint --limit 20\n  \
    qn usage streams --from 7d --group-by stream")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: UsageCmd,
}

#[derive(Debug, Subcommand)]
pub enum UsageCmd {
    /// Aggregate usage summary for the billing period (or --from/--to).
    Summary(Range),
    /// Per-endpoint usage breakdown.
    ByEndpoint(Range),
    /// Per-RPC-method usage breakdown.
    ByMethod(Range),
    /// Per-chain usage breakdown.
    ByChain(Range),
    /// Per-tag usage breakdown.
    ByTag(Range),
    /// Credit totals over a time window.
    Credits(CreditsArgs),
    /// RPC totals over a time window, optionally grouped and filtered.
    Rpc(RpcArgs),
    /// Streams totals over a time window, optionally grouped by stream.
    Streams(StreamsArgs),
    /// Webhooks totals over a time window, optionally grouped by webhook.
    Webhooks(WebhooksArgs),
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn usage credits --from 24h\n  \
    qn usage credits --from 2026-01-01T00:00:00Z --to 2026-02-01T00:00:00Z\n  \
    qn usage credits --cursor eyJvZmZzZXQiOjEwMH0")]
pub struct CreditsArgs {
    #[command(flatten)]
    pub window: WindowArgs,
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn usage rpc --from 1h\n  \
    qn usage rpc --from 7d --group-by method,endpoint --limit 20\n  \
    qn usage rpc --from 24h --chain ethereum --network mainnet --method eth_call\n  \
    qn usage rpc --cursor eyJvZmZzZXQiOjEwMH0")]
pub struct RpcArgs {
    #[command(flatten)]
    pub window: WindowArgs,
    /// Group rows by these dimensions (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',', value_parser = ["method", "endpoint", "chain", "network"])]
    pub group_by: Vec<String>,
    #[command(flatten)]
    pub filters: RpcFilterArgs,
}

/// Filters shared by `usage rpc` and `metrics traffic`.
#[derive(Debug, ClapArgs)]
pub struct RpcFilterArgs {
    /// Only these endpoint subdomains (repeatable).
    #[arg(long = "endpoint-name", value_name = "NAME")]
    pub endpoint_names: Vec<String>,
    /// Only these chains (repeatable).
    #[arg(long = "chain")]
    pub chains: Vec<String>,
    /// Only these networks (repeatable).
    #[arg(long = "network")]
    pub networks: Vec<String>,
    /// Only these RPC methods (repeatable).
    #[arg(long = "method")]
    pub methods: Vec<String>,
    /// Only endpoints carrying these numeric tag ids (repeatable).
    #[arg(long = "tag-id")]
    pub tag_ids: Vec<String>,
    /// Only these numeric add-on ids (repeatable).
    #[arg(long = "addon-id")]
    pub addon_ids: Vec<String>,
}

impl RpcFilterArgs {
    pub fn is_empty(&self) -> bool {
        self.endpoint_names.is_empty()
            && self.chains.is_empty()
            && self.networks.is_empty()
            && self.methods.is_empty()
            && self.tag_ids.is_empty()
            && self.addon_ids.is_empty()
    }
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn usage streams --from 7d\n  \
    qn usage streams --from 7d --group-by stream\n  \
    qn usage streams --from 24h --stream-id st-1")]
pub struct StreamsArgs {
    #[command(flatten)]
    pub window: WindowArgs,
    /// Group rows by stream.
    #[arg(long, value_delimiter = ',', value_parser = ["stream"])]
    pub group_by: Vec<String>,
    /// Only this stream.
    #[arg(long, value_name = "STREAM_ID")]
    pub stream_id: Option<String>,
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn usage webhooks --from 7d\n  \
    qn usage webhooks --from 7d --group-by webhook\n  \
    qn usage webhooks --from 24h --webhook-id wh-1")]
pub struct WebhooksArgs {
    #[command(flatten)]
    pub window: WindowArgs,
    /// Group rows by webhook.
    #[arg(long, value_delimiter = ',', value_parser = ["webhook"])]
    pub group_by: Vec<String>,
    /// Only this webhook.
    #[arg(long, value_name = "WEBHOOK_ID")]
    pub webhook_id: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct Range {
    /// Start time (RFC-3339, relative like `7d`, or `now`). Omit for account-to-date.
    #[arg(long)]
    pub from: Option<String>,
    /// End time. Defaults to now.
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
        UsageCmd::Credits(a) => credits(a, ctx).await,
        UsageCmd::Rpc(a) => rpc(a, ctx).await,
        UsageCmd::Streams(a) => streams(a, ctx).await,
        UsageCmd::Webhooks(a) => webhooks(a, ctx).await,
    }
}

async fn credits(a: CreditsArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a.window.resolve(false)?;
    let req = GetUsageCreditsSummaryRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        cursor: w.cursor,
        limit: w.limit,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_usage_credits_summary(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &CreditsSummaryView(resp))
}

async fn rpc(a: RpcArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a
        .window
        .resolve(!a.group_by.is_empty() || !a.filters.is_empty())?;
    let f = a.filters;
    let req = GetUsageRpcSummaryRequest {
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
        ctx.sdk.admin.get_usage_rpc_summary(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &RpcSummaryView(resp))
}

async fn streams(a: StreamsArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a
        .window
        .resolve(!a.group_by.is_empty() || a.stream_id.is_some())?;
    let req = GetUsageStreamsSummaryRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        cursor: w.cursor,
        limit: w.limit,
        group_by: non_empty(a.group_by),
        stream_id: a.stream_id,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_usage_streams_summary(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &StreamsSummaryView(resp))
}

async fn webhooks(a: WebhooksArgs, ctx: Ctx) -> Result<(), CliError> {
    let w = a
        .window
        .resolve(!a.group_by.is_empty() || a.webhook_id.is_some())?;
    let req = GetUsageWebhooksSummaryRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        cursor: w.cursor,
        limit: w.limit,
        group_by: non_empty(a.group_by),
        webhook_id: a.webhook_id,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_usage_webhooks_summary(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &WebhooksSummaryView(resp))
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
struct CreditsSummaryView(quicknode_sdk::admin::GetUsageCreditsSummaryResponse);

impl Render for CreditsSummaryView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        let breakdown = d
            .breakdown
            .as_deref()
            .map(|rows| breakdown_table(rows, &[], &["CREDITS"], |r| vec![Cell::new(r.credits)]));
        write_summary(
            w,
            ctx,
            SummaryPage {
                from: &d.from_time,
                to: &d.to_time,
                totals: vec![("credits", Cell::new(d.totals.credits))],
                breakdown,
                next_cursor: d.next_cursor.as_deref(),
            },
        )
    }
}

#[derive(Serialize)]
struct RpcSummaryView(quicknode_sdk::admin::GetUsageRpcSummaryResponse);

impl Render for RpcSummaryView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        let t = &d.totals;
        let breakdown = d.breakdown.as_deref().map(|rows| {
            breakdown_table(
                rows,
                &[
                    ("METHOD", |r: &UsageRpcSummaryRow| dim(&r.method)),
                    ("ENDPOINT", |r: &UsageRpcSummaryRow| dim(&r.endpoint)),
                    ("CHAIN", |r: &UsageRpcSummaryRow| dim(&r.chain)),
                    ("NETWORK", |r: &UsageRpcSummaryRow| dim(&r.network)),
                ],
                &[
                    "REQUESTS",
                    "BYTES",
                    "RPC_CREDITS",
                    "DATA_CREDITS",
                    "CREDITS",
                ],
                |r| {
                    vec![
                        opt_cell(&r.requests),
                        Cell::new(r.bytes),
                        Cell::new(r.rpc_credits),
                        Cell::new(r.data_credits),
                        Cell::new(r.credits),
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
                    ("requests", Cell::new(t.requests)),
                    ("bytes", Cell::new(t.bytes)),
                    ("rpc_credits", Cell::new(t.rpc_credits)),
                    ("data_credits", Cell::new(t.data_credits)),
                    ("credits", Cell::new(t.credits)),
                ],
                breakdown,
                next_cursor: d.next_cursor.as_deref(),
            },
        )
    }
}

#[derive(Serialize)]
struct StreamsSummaryView(quicknode_sdk::admin::GetUsageStreamsSummaryResponse);

impl Render for StreamsSummaryView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        let t = &d.totals;
        let breakdown = d.breakdown.as_deref().map(|rows| {
            breakdown_table(
                rows,
                &[("STREAM", |r: &UsageStreamsSummaryRow| dim(&r.stream))],
                &["BLOCKS", "BYTES", "BLOCK_CREDITS"],
                |r| {
                    vec![
                        Cell::new(r.blocks),
                        Cell::new(r.bytes),
                        Cell::new(r.block_credits),
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
                    ("blocks", Cell::new(t.blocks)),
                    ("bytes", Cell::new(t.bytes)),
                    ("block_credits", Cell::new(t.block_credits)),
                ],
                breakdown,
                next_cursor: d.next_cursor.as_deref(),
            },
        )
    }
}

#[derive(Serialize)]
struct WebhooksSummaryView(quicknode_sdk::admin::GetUsageWebhooksSummaryResponse);

impl Render for WebhooksSummaryView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        let t = &d.totals;
        let breakdown = d.breakdown.as_deref().map(|rows| {
            breakdown_table(
                rows,
                &[("WEBHOOK", |r: &UsageWebhooksSummaryRow| dim(&r.webhook))],
                &["DELIVERIES", "CREDITS"],
                |r| vec![Cell::new(r.deliveries), Cell::new(r.credits)],
            )
        });
        write_summary(
            w,
            ctx,
            SummaryPage {
                from: &d.from_time,
                to: &d.to_time,
                totals: vec![
                    ("deliveries", Cell::new(t.deliveries)),
                    ("credits", Cell::new(t.credits)),
                ],
                breakdown,
                next_cursor: d.next_cursor.as_deref(),
            },
        )
    }
}

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
        t.add_row(vec![Cell::new("overages"), opt_cell(&data.overages)]);
        // The API reports the window as unix seconds; show RFC-3339 like the
        // --from/--to flags accept. Out-of-range values fall back to the raw
        // number rather than erroring a read-only command.
        let time_cell = |ts: i64| match time_arg::ParsedTime::from_unix(ts) {
            Some(t) => Cell::new(t.to_rfc3339()),
            None => Cell::new(ts),
        };
        t.add_row(vec![Cell::new("start_time"), time_cell(data.start_time)]);
        t.add_row(vec![Cell::new("end_time"), time_cell(data.end_time)]);
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
        let mut headers = vec!["ENDPOINT", "CHAIN", "NETWORK", "CREDITS"];
        if ctx.wide {
            headers.extend(["RPC_CREDITS", "DATA_CREDITS", "BYTES"]);
        }
        set_header_bold(&mut t, ctx, headers);
        for e in &data.endpoints {
            let mut row = vec![
                Cell::new(&e.name),
                opt_cell(&e.chain),
                opt_cell(&e.network),
                Cell::new(e.credits_used),
            ];
            if ctx.wide {
                row.push(opt_cell(&e.rpc_credits));
                row.push(opt_cell(&e.data_credits));
                row.push(opt_cell(&e.bytes));
            }
            t.add_row(row);
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
        let mut headers = vec!["METHOD", "CHAIN", "NETWORK", "CREDITS", "ARCHIVE"];
        if ctx.wide {
            headers.push("BYTES");
        }
        set_header_bold(&mut t, ctx, headers);
        for m in &data.methods {
            let mut row = vec![
                Cell::new(&m.method_name),
                opt_cell(&m.chain),
                opt_cell(&m.network),
                Cell::new(m.credits_used),
                bool_cell(m.archive),
            ];
            if ctx.wide {
                row.push(opt_cell(&m.bytes));
            }
            t.add_row(row);
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
        let mut headers = vec!["CHAIN", "CREDITS"];
        if ctx.wide {
            headers.extend(["RPC_CREDITS", "DATA_CREDITS", "BYTES"]);
        }
        set_header_bold(&mut t, ctx, headers);
        for c in &data.chains {
            let mut row = vec![Cell::new(&c.name), Cell::new(c.credits_used)];
            if ctx.wide {
                row.push(opt_cell(&c.rpc_credits));
                row.push(opt_cell(&c.data_credits));
                row.push(opt_cell(&c.bytes));
            }
            t.add_row(row);
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
        let mut headers = vec!["TAG_ID", "LABEL", "CREDITS", "REQUESTS"];
        if ctx.wide {
            headers.extend(["RPC_CREDITS", "DATA_CREDITS", "BYTES"]);
        }
        set_header_bold(&mut t, ctx, headers);
        for tg in &data.tags {
            let mut row = vec![
                opt_cell(&tg.tag_id),
                Cell::new(&tg.label),
                Cell::new(tg.credits_used),
                Cell::new(tg.requests),
            ];
            if ctx.wide {
                row.push(opt_cell(&tg.rpc_credits));
                row.push(opt_cell(&tg.data_credits));
                row.push(opt_cell(&tg.bytes));
            }
            t.add_row(row);
        }
        write_table(w, &t)
    }
}
