//! `qn endpoint …` — manage RPC endpoints.

use clap::{Args as ClapArgs, Subcommand};
use quicknode_sdk::admin::{
    CreateEndpointRequest, GetEndpointLogsRequest, GetEndpointMetricsRequest, GetEndpointsRequest,
    UpdateEndpointRequest, UpdateEndpointStatusRequest,
};

use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::time_arg;

mod ratelimit;
pub(crate) mod render;
mod security;
mod tag;

pub use ratelimit::RateLimitCmd;
pub use security::SecurityCmd;
pub use tag::TagCmd;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: EndpointCmd,
}

#[derive(Debug, Subcommand)]
pub enum EndpointCmd {
    /// List endpoints on the account.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Create a new endpoint on a chain/network.
    Create(CreateArgs),
    /// Show full details for a single endpoint.
    Show { id: String },
    /// Update an endpoint's label.
    Update(UpdateArgs),
    /// Archive an endpoint (irreversible from the CLI).
    Archive(ArchiveArgs),
    /// Pause an endpoint (stops accepting requests).
    Pause { id: String },
    /// Resume a paused endpoint.
    Resume { id: String },
    /// Show the HTTP and WebSocket URLs for an endpoint.
    Urls { id: String },
    /// Fetch request logs for an endpoint.
    Logs(LogsArgs),
    /// Fetch a single request log's full request/response payloads.
    LogDetails {
        /// Endpoint id.
        id: String,
        /// Request id (UUID from the logs listing).
        request_id: String,
    },
    /// Fetch metric series for an endpoint.
    Metrics(MetricsArgs),
    /// Enable multichain on an endpoint.
    EnableMultichain { id: String },
    /// Disable multichain on an endpoint.
    DisableMultichain { id: String },

    /// Manage tags on an endpoint (use `qn tag` for account-wide tag management).
    #[command(subcommand)]
    Tag(TagCmd),

    /// Manage endpoint security settings (tokens, referrers, IPs, JWTs, ...).
    #[command(subcommand)]
    Security(SecurityCmd),

    /// Manage rate limits on an endpoint.
    #[command(subcommand)]
    RateLimit(RateLimitCmd),
}

#[derive(Debug, ClapArgs)]
pub struct ListArgs {
    /// Page size (max 100).
    #[arg(long)]
    pub limit: Option<i32>,
    /// Offset for pagination.
    #[arg(long)]
    pub offset: Option<i32>,
    /// Free-text search.
    #[arg(long)]
    pub search: Option<String>,
    /// Sort key (e.g. `created_at`, `label`).
    #[arg(long)]
    pub sort_by: Option<String>,
    /// Sort direction.
    #[arg(long, value_parser = ["asc", "desc"])]
    pub sort_direction: Option<String>,
    /// Filter by network (repeatable).
    #[arg(long = "network")]
    pub networks: Vec<String>,
    /// Filter by status (repeatable).
    #[arg(long = "status")]
    pub statuses: Vec<String>,
    /// Filter by label (repeatable).
    #[arg(long = "label")]
    pub labels: Vec<String>,
    /// Dedicated-only filter.
    #[arg(long)]
    pub dedicated: Option<bool>,
    /// Flat-rate filter.
    #[arg(long = "flat-rate")]
    pub is_flat_rate: Option<bool>,
    /// Filter by tag id (repeatable).
    #[arg(long = "tag-id")]
    pub tag_ids: Vec<i32>,
    /// Filter by tag label (repeatable).
    #[arg(long = "tag")]
    pub tag_labels: Vec<String>,
}

#[derive(Debug, ClapArgs)]
pub struct CreateArgs {
    /// Blockchain (e.g. `ethereum`, `solana`).
    #[arg(long)]
    pub chain: Option<String>,
    /// Network (e.g. `mainnet`).
    #[arg(long)]
    pub network: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct UpdateArgs {
    /// Endpoint id.
    pub id: String,
    /// New label.
    #[arg(long)]
    pub label: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct ArchiveArgs {
    /// Endpoint id.
    pub id: String,
}

#[derive(Debug, ClapArgs)]
pub struct LogsArgs {
    /// Endpoint id.
    pub id: String,
    /// Start of the window (RFC-3339, relative duration like `1h`, or `now`).
    #[arg(long)]
    pub from: String,
    /// End of the window. Defaults to now.
    #[arg(long, default_value = "now")]
    pub to: String,
    /// Page size.
    #[arg(long)]
    pub limit: Option<i32>,
    /// Pagination cursor (from a previous page's `next_at`).
    #[arg(long)]
    pub next_at: Option<String>,
    /// Include full request/response bodies inline.
    #[arg(long)]
    pub details: bool,
}

#[derive(Debug, ClapArgs)]
pub struct MetricsArgs {
    /// Endpoint id.
    pub id: String,
    /// Metric name (e.g. `method_calls_over_time`, `response_status_breakdown`).
    #[arg(long)]
    pub metric: String,
    /// Period (`hour`, `day`, `week`, `month`).
    #[arg(long, value_parser = ["hour", "day", "week", "month"])]
    pub period: String,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        EndpointCmd::List(a) => list(a, ctx).await,
        EndpointCmd::Create(a) => create(a, ctx).await,
        EndpointCmd::Show { id } => show(&id, ctx).await,
        EndpointCmd::Update(a) => update(a, ctx).await,
        EndpointCmd::Archive(a) => archive(a, ctx).await,
        EndpointCmd::Pause { id } => set_status(&id, "paused", ctx).await,
        EndpointCmd::Resume { id } => set_status(&id, "active", ctx).await,
        EndpointCmd::Urls { id } => urls(&id, ctx).await,
        EndpointCmd::Logs(a) => logs(a, ctx).await,
        EndpointCmd::LogDetails { id, request_id } => log_details(&id, &request_id, ctx).await,
        EndpointCmd::Metrics(a) => metrics(a, ctx).await,
        EndpointCmd::EnableMultichain { id } => enable_multichain(&id, ctx).await,
        EndpointCmd::DisableMultichain { id } => disable_multichain(&id, ctx).await,
        EndpointCmd::Tag(c) => tag::run(c, ctx).await,
        EndpointCmd::Security(c) => security::run(c, ctx).await,
        EndpointCmd::RateLimit(c) => ratelimit::run(c, ctx).await,
    }
}

async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let mut req = GetEndpointsRequest {
        limit: a.limit,
        offset: a.offset,
        search: a.search,
        sort_by: a.sort_by,
        sort_direction: a.sort_direction,
        dedicated: a.dedicated,
        is_flat_rate: a.is_flat_rate,
        ..Default::default()
    };
    if !a.networks.is_empty() {
        req.networks = Some(a.networks);
    }
    if !a.statuses.is_empty() {
        req.statuses = Some(a.statuses);
    }
    if !a.labels.is_empty() {
        req.labels = Some(a.labels);
    }
    if !a.tag_ids.is_empty() {
        req.tag_ids = Some(a.tag_ids);
    }
    if !a.tag_labels.is_empty() {
        req.tag_labels = Some(a.tag_labels);
    }
    let resp = ctx.sdk.admin.get_endpoints(&req).await?;
    crate::output::emit(&ctx.out, &render::EndpointsView(resp))
}

async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = CreateEndpointRequest {
        chain: a.chain,
        network: a.network,
    };
    let resp = ctx.sdk.admin.create_endpoint(&req).await?;
    ctx.out
        .note(&format!("✓ Created endpoint {}", resp.data.id));
    crate::output::emit(&ctx.out, &render::SingleEndpointView(resp.data))
}

async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.show_endpoint(id).await?;
    let data = resp
        .data
        .ok_or_else(|| CliError::Arg(format!("endpoint {id} not found")))?;
    crate::output::emit(&ctx.out, &render::SingleEndpointView(data))
}

async fn update(a: UpdateArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = UpdateEndpointRequest { label: a.label };
    ctx.sdk.admin.update_endpoint(&a.id, &req).await?;
    ctx.out.note(&format!("✓ Updated endpoint {}", a.id));
    Ok(())
}

async fn archive(a: ArchiveArgs, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Archive endpoint {}?", a.id))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.admin.archive_endpoint(&a.id).await?;
    ctx.out.note(&format!("✓ Archived endpoint {}", a.id));
    Ok(())
}

async fn set_status(id: &str, status: &str, ctx: Ctx) -> Result<(), CliError> {
    let req = UpdateEndpointStatusRequest {
        status: status.to_string(),
    };
    ctx.sdk.admin.update_endpoint_status(id, &req).await?;
    let verb = if status == "paused" {
        "Paused"
    } else {
        "Resumed"
    };
    ctx.out.note(&format!("✓ {verb} endpoint {id}"));
    Ok(())
}

async fn urls(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.get_endpoint_urls(id).await?;
    let data = resp
        .data
        .ok_or_else(|| CliError::Arg(format!("API returned no URL data for endpoint {id}")))?;
    crate::output::emit(&ctx.out, &render::EndpointUrlsView(data))
}

async fn logs(a: LogsArgs, ctx: Ctx) -> Result<(), CliError> {
    let from = time_arg::parse_rfc3339(&a.from)?;
    let to = time_arg::parse_rfc3339(&a.to)?;
    let req = GetEndpointLogsRequest {
        from,
        to,
        limit: a.limit,
        next_at: a.next_at,
        include_details: Some(a.details),
    };
    let resp = ctx.sdk.admin.get_endpoint_logs(&a.id, &req).await?;
    crate::output::emit(&ctx.out, &render::EndpointLogsView(resp))
}

async fn log_details(id: &str, request_id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.get_log_details(id, request_id).await?;
    crate::output::emit(&ctx.out, &render::LogDetailsView(resp))
}

async fn metrics(a: MetricsArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = GetEndpointMetricsRequest {
        period: a.period,
        metric: a.metric,
    };
    let resp = ctx.sdk.admin.get_endpoint_metrics(&a.id, &req).await?;
    crate::output::emit(&ctx.out, &render::EndpointMetricsView(resp))
}

async fn enable_multichain(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.admin.enable_multichain(id).await?;
    ctx.out.note(&format!("✓ Enabled multichain on {id}"));
    Ok(())
}

async fn disable_multichain(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.admin.disable_multichain(id).await?;
    ctx.out.note(&format!("✓ Disabled multichain on {id}"));
    Ok(())
}
