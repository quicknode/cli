//! `qn stream …` — blockchain data streams.
//!
//! Stream `create` is highly configurable. To keep the CLI usable we expose
//! the common fields directly (name, network, dataset, range, region, plan,
//! webhook url) and provide `--config-file` to load a full JSON
//! `CreateStreamParams` from disk for advanced cases (S3/Azure/Postgres/Kafka
//! destinations, filter functions, extra destinations, etc.).

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::streams::{
    CreateStreamParams, DestinationAttributes, FilterLanguage, ListStreamsParams, StreamDataset,
    StreamRegion, StreamStatus, TestFilterParams, UpdateStreamParams, WebhookAttributes,
};
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_typed, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: StreamCmd,
}

#[derive(Debug, Subcommand)]
pub enum StreamCmd {
    /// List streams on the account.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Create a stream (webhook destination). For non-webhook destinations, use --config-file.
    Create(Box<CreateArgs>),
    /// Show a stream's full configuration and current state.
    Show { id: String },
    /// Update editable fields on a stream.
    Update(UpdateArgs),
    /// Delete a stream.
    Delete { id: String },
    /// Delete every stream on the account.
    DeleteAll,
    /// Activate (resume) a stream.
    Activate { id: String },
    /// Pause a stream.
    Pause { id: String },
    /// Run a filter against a block without creating a stream.
    TestFilter(TestFilterArgs),
    /// Count of currently enabled streams.
    EnabledCount {
        /// Optional stream-type filter.
        #[arg(long)]
        r#type: Option<String>,
    },
}

#[derive(Debug, ClapArgs)]
pub struct ListArgs {
    #[arg(long)]
    pub limit: Option<i64>,
    #[arg(long)]
    pub offset: Option<i64>,
    #[arg(long)]
    pub order_by: Option<String>,
    #[arg(long, value_parser = ["asc", "desc"])]
    pub order_direction: Option<String>,
    #[arg(long)]
    pub stream_type: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct CreateArgs {
    /// Load full `CreateStreamParams` from a JSON file. When supplied, all
    /// other --flags are ignored.
    #[arg(long, conflicts_with_all = ["name", "network", "dataset", "start", "end", "region", "plan", "webhook"])]
    pub config_file: Option<PathBuf>,

    /// Stream name.
    #[arg(long)]
    pub name: Option<String>,
    /// Network (e.g. `ethereum-mainnet`).
    #[arg(long)]
    pub network: Option<String>,
    /// Dataset (snake_case).
    #[arg(long, value_enum)]
    pub dataset: Option<DatasetArg>,
    /// Start block.
    #[arg(long)]
    pub start: Option<i64>,
    /// End block (`-1` for continuous).
    #[arg(long, allow_hyphen_values = true)]
    pub end: Option<i64>,
    /// Region.
    #[arg(long, value_enum)]
    pub region: Option<RegionArg>,
    /// Billing plan slug (optional).
    #[arg(long)]
    pub plan: Option<String>,
    /// Webhook URL for the primary destination.
    #[arg(long)]
    pub webhook: Option<String>,
    /// Webhook security token (32-byte minimum). Optional.
    #[arg(long)]
    pub webhook_security_token: Option<String>,

    /// dataset_batch_size (defaults to 1).
    #[arg(long)]
    pub batch_size: Option<i64>,
    /// fix_block_reorgs (0/1).
    #[arg(long)]
    pub fix_block_reorgs: Option<i32>,
    /// elastic_batch_enabled.
    #[arg(long)]
    pub elastic_batch_enabled: Option<bool>,
    /// Notification email.
    #[arg(long)]
    pub notification_email: Option<String>,
    /// Initial status.
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,
    /// Filter function source (raw text). Will be base64-encoded.
    #[arg(long, conflicts_with = "filter_file")]
    pub filter: Option<String>,
    /// Path to a filter function file (will be base64-encoded).
    #[arg(long)]
    pub filter_file: Option<PathBuf>,
    /// Filter language.
    #[arg(long, value_enum)]
    pub filter_language: Option<FilterLanguageArg>,
    /// Threshold fetch buffer (server default applies if omitted).
    #[arg(long)]
    pub threshold_fetch_buffer: Option<i64>,
    /// Keep distance from tip (server default applies if omitted).
    #[arg(long)]
    pub keep_distance_from_tip: Option<i64>,
}

#[derive(Debug, ClapArgs)]
pub struct UpdateArgs {
    pub id: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,
    #[arg(long)]
    pub notification_email: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct TestFilterArgs {
    #[arg(long)]
    pub network: String,
    #[arg(long, value_enum)]
    pub dataset: DatasetArg,
    #[arg(long)]
    pub block: String,
    /// Filter function source (raw text). Will be base64-encoded.
    #[arg(long, conflicts_with = "filter_file")]
    pub filter: Option<String>,
    #[arg(long)]
    pub filter_file: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub filter_language: Option<FilterLanguageArg>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RegionArg {
    UsaEast,
    EuropeCentral,
    AsiaEast,
}

impl From<RegionArg> for StreamRegion {
    fn from(r: RegionArg) -> Self {
        match r {
            RegionArg::UsaEast => StreamRegion::UsaEast,
            RegionArg::EuropeCentral => StreamRegion::EuropeCentral,
            RegionArg::AsiaEast => StreamRegion::AsiaEast,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DatasetArg {
    Block,
    BlockWithReceipts,
    Transactions,
    Logs,
    Receipts,
    TraceBlocks,
    DebugTraces,
    BlockWithReceiptsDebugTrace,
    BlockWithReceiptsTraceBlock,
    BlobSidecars,
    ProgramsWithLogs,
    Ledger,
    Events,
    Orders,
    Trades,
    BookUpdates,
    Twap,
    WriterActions,
}

impl From<DatasetArg> for StreamDataset {
    fn from(d: DatasetArg) -> Self {
        match d {
            DatasetArg::Block => StreamDataset::Block,
            DatasetArg::BlockWithReceipts => StreamDataset::BlockWithReceipts,
            DatasetArg::Transactions => StreamDataset::Transactions,
            DatasetArg::Logs => StreamDataset::Logs,
            DatasetArg::Receipts => StreamDataset::Receipts,
            DatasetArg::TraceBlocks => StreamDataset::TraceBlocks,
            DatasetArg::DebugTraces => StreamDataset::DebugTraces,
            DatasetArg::BlockWithReceiptsDebugTrace => StreamDataset::BlockWithReceiptsDebugTrace,
            DatasetArg::BlockWithReceiptsTraceBlock => StreamDataset::BlockWithReceiptsTraceBlock,
            DatasetArg::BlobSidecars => StreamDataset::BlobSidecars,
            DatasetArg::ProgramsWithLogs => StreamDataset::ProgramsWithLogs,
            DatasetArg::Ledger => StreamDataset::Ledger,
            DatasetArg::Events => StreamDataset::Events,
            DatasetArg::Orders => StreamDataset::Orders,
            DatasetArg::Trades => StreamDataset::Trades,
            DatasetArg::BookUpdates => StreamDataset::BookUpdates,
            DatasetArg::Twap => StreamDataset::Twap,
            DatasetArg::WriterActions => StreamDataset::WriterActions,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StatusArg {
    Active,
    Paused,
    Terminated,
    Completed,
    Blocked,
}

impl From<StatusArg> for StreamStatus {
    fn from(s: StatusArg) -> Self {
        match s {
            StatusArg::Active => StreamStatus::Active,
            StatusArg::Paused => StreamStatus::Paused,
            StatusArg::Terminated => StreamStatus::Terminated,
            StatusArg::Completed => StreamStatus::Completed,
            StatusArg::Blocked => StreamStatus::Blocked,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FilterLanguageArg {
    Javascript,
    Go,
    Wasm,
}

impl From<FilterLanguageArg> for FilterLanguage {
    fn from(f: FilterLanguageArg) -> Self {
        match f {
            FilterLanguageArg::Javascript => FilterLanguage::Javascript,
            FilterLanguageArg::Go => FilterLanguage::Go,
            FilterLanguageArg::Wasm => FilterLanguage::Wasm,
        }
    }
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        StreamCmd::List(a) => list(a, ctx).await,
        StreamCmd::Create(a) => create(*a, ctx).await,
        StreamCmd::Show { id } => show(&id, ctx).await,
        StreamCmd::Update(a) => update(a, ctx).await,
        StreamCmd::Delete { id } => delete(&id, ctx).await,
        StreamCmd::DeleteAll => delete_all(ctx).await,
        StreamCmd::Activate { id } => activate(&id, ctx).await,
        StreamCmd::Pause { id } => pause(&id, ctx).await,
        StreamCmd::TestFilter(a) => test_filter(a, ctx).await,
        StreamCmd::EnabledCount { r#type } => enabled_count(r#type, ctx).await,
    }
}

async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = ListStreamsParams {
        stream_type: a.stream_type,
        offset: a.offset,
        limit: a.limit,
        order_by: a.order_by,
        order_direction: a.order_direction,
    };
    let resp = ctx.sdk.streams.list_streams(&params).await?;
    crate::output::emit(&ctx.out, &StreamsListView(resp))
}

async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = if let Some(path) = a.config_file {
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str::<CreateStreamParams>(&text)?
    } else {
        build_create_params(a)?
    };
    let stream = ctx.sdk.streams.create_stream(&params).await?;
    ctx.out.note(&format!("✓ Created stream {}", stream.id));
    crate::output::emit(&ctx.out, &StreamView(stream))
}

fn build_create_params(a: CreateArgs) -> Result<CreateStreamParams, CliError> {
    let name = a
        .name
        .ok_or_else(|| CliError::Arg("--name is required".into()))?;
    let network = a
        .network
        .ok_or_else(|| CliError::Arg("--network is required".into()))?;
    let dataset = a
        .dataset
        .ok_or_else(|| CliError::Arg("--dataset is required".into()))?;
    let start = a
        .start
        .ok_or_else(|| CliError::Arg("--start is required".into()))?;
    let end = a
        .end
        .ok_or_else(|| CliError::Arg("--end is required (-1 for continuous)".into()))?;
    let region = a
        .region
        .ok_or_else(|| CliError::Arg("--region is required".into()))?;
    let url = a.webhook.ok_or_else(|| {
        CliError::Arg("--webhook is required (or use --config-file for other destinations)".into())
    })?;

    let filter_function = match (a.filter, a.filter_file) {
        (Some(s), None) => Some(STANDARD.encode(s)),
        (None, Some(p)) => Some(STANDARD.encode(std::fs::read(&p)?)),
        (None, None) => None,
        (Some(_), Some(_)) => {
            return Err(CliError::Arg(
                "supply only one of --filter or --filter-file".into(),
            ));
        }
    };

    Ok(CreateStreamParams {
        name,
        region: region.into(),
        network,
        dataset: dataset.into(),
        start_range: start,
        end_range: end,
        destination_attributes: DestinationAttributes::Webhook(WebhookAttributes {
            url,
            max_retry: 3,
            retry_interval_sec: 1,
            post_timeout_sec: 10,
            compression: None,
            security_token: a.webhook_security_token,
        }),
        plan: a.plan,
        threshold_fetch_buffer: a.threshold_fetch_buffer,
        dataset_batch_size: a.batch_size.unwrap_or(1),
        max_batch_size: None,
        max_buffer_range_size: None,
        max_buffer_processing_workers: None,
        keep_distance_from_tip: a.keep_distance_from_tip,
        filter_function,
        filter_language: a.filter_language.map(Into::into),
        address_book_config: None,
        include_stream_metadata: None,
        product_type: None,
        status: a.status.map(Into::into),
        notification_email: a.notification_email,
        charge_min_cap: None,
        fix_block_reorgs: a.fix_block_reorgs,
        elastic_batch_enabled: a.elastic_batch_enabled.unwrap_or(false),
        extra_destinations: None,
    })
}

async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let s = ctx.sdk.streams.get_stream(id).await?;
    crate::output::emit(&ctx.out, &StreamView(s))
}

async fn update(a: UpdateArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = UpdateStreamParams {
        name: a.name,
        status: a.status.map(Into::into),
        notification_email: a.notification_email,
        ..Default::default()
    };
    let s = ctx.sdk.streams.update_stream(&a.id, &params).await?;
    ctx.out.note(&format!("✓ Updated stream {}", a.id));
    crate::output::emit(&ctx.out, &StreamView(s))
}

async fn delete(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete stream {id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.streams.delete_stream(id).await?;
    ctx.out.note(&format!("✓ Deleted stream {id}"));
    Ok(())
}

async fn delete_all(ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Severe, cfg)? {
        true => true,
        false => prompt_typed(
            "Type 'delete-all' to delete EVERY stream on the account",
            "delete-all",
        )?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.streams.delete_all_streams().await?;
    ctx.out.note("✓ Deleted all streams");
    Ok(())
}

async fn activate(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.streams.activate_stream(id).await?;
    ctx.out.note(&format!("✓ Activated stream {id}"));
    Ok(())
}

async fn pause(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.streams.pause_stream(id).await?;
    ctx.out.note(&format!("✓ Paused stream {id}"));
    Ok(())
}

async fn test_filter(a: TestFilterArgs, ctx: Ctx) -> Result<(), CliError> {
    let filter_function = match (a.filter, a.filter_file) {
        (Some(s), None) => STANDARD.encode(s),
        (None, Some(p)) => STANDARD.encode(std::fs::read(&p)?),
        (None, None) => {
            return Err(CliError::Arg("supply --filter or --filter-file".into()));
        }
        (Some(_), Some(_)) => {
            return Err(CliError::Arg(
                "supply only one of --filter or --filter-file".into(),
            ));
        }
    };
    let params = TestFilterParams {
        network: a.network,
        dataset: a.dataset.into(),
        block: a.block,
        filter_function,
        filter_language: a.filter_language.map(Into::into),
        address_book_config: None,
    };
    let resp = ctx.sdk.streams.test_filter(&params).await?;
    crate::output::emit(&ctx.out, &TestFilterView(resp))
}

async fn enabled_count(stream_type: Option<String>, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx
        .sdk
        .streams
        .get_enabled_count(stream_type.as_deref())
        .await?;
    if ctx.out.format.is_structured() {
        crate::output::emit(&ctx.out, &resp)
    } else {
        println!("{}", resp.total);
        Ok(())
    }
}

// ----- renderers ----- //

#[derive(Serialize)]
struct StreamsListView(quicknode_sdk::streams::ListStreamsResponse);

impl Render for StreamsListView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "NAME", "STATUS", "NETWORK", "DATASET", "REGION"],
        );
        for s in &self.0.data {
            t.add_row(vec![
                Cell::new(&s.id),
                Cell::new(&s.name),
                Cell::new(&s.status),
                Cell::new(&s.network),
                Cell::new(&s.dataset),
                Cell::new(&s.region),
            ]);
        }
        write_table(w, &t)?;
        writeln!(
            w,
            "showing {}–{} of {}",
            self.0.page_info.offset + 1,
            (self.0.page_info.offset + self.0.data.len() as i64).min(self.0.page_info.total),
            self.0.page_info.total
        )
    }
}

#[derive(Serialize)]
struct StreamView(quicknode_sdk::streams::Stream);

impl Render for StreamView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let s = &self.0;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(&s.id)]);
        t.add_row(vec![Cell::new("name"), Cell::new(&s.name)]);
        t.add_row(vec![Cell::new("status"), Cell::new(&s.status)]);
        t.add_row(vec![Cell::new("network"), Cell::new(&s.network)]);
        t.add_row(vec![Cell::new("dataset"), Cell::new(&s.dataset)]);
        t.add_row(vec![Cell::new("region"), Cell::new(&s.region)]);
        t.add_row(vec![Cell::new("start_range"), Cell::new(s.start_range)]);
        t.add_row(vec![Cell::new("end_range"), Cell::new(s.end_range)]);
        t.add_row(vec![Cell::new("created_at"), Cell::new(&s.created_at)]);
        t.add_row(vec![Cell::new("updated_at"), Cell::new(&s.updated_at)]);
        t.add_row(vec![Cell::new("plan"), opt_cell(&s.plan)]);
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct TestFilterView(quicknode_sdk::streams::TestFilterResponse);

impl Render for TestFilterView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        if !self.0.logs.is_empty() {
            writeln!(w, "== logs ==")?;
            for line in &self.0.logs {
                writeln!(w, "{line}")?;
            }
        }
        writeln!(w, "== result ==")?;
        writeln!(w, "{}", self.0.result)
    }
}

impl Render for quicknode_sdk::streams::EnabledCountResponse {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        writeln!(w, "{}", self.total)
    }
}
