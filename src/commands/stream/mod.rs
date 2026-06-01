//! `qn stream …` — blockchain data streams.
//!
//! Stream `create` is highly configurable. To keep the CLI usable we expose
//! the common fields directly (name, network, dataset, range, region, plan,
//! webhook url) and provide `--config-file` to load a full JSON
//! `CreateStreamParams` from disk for advanced cases (S3/Azure/Postgres/Kafka
//! destinations, filter functions, extra destinations, etc.).

mod actions;
mod render;

use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use quicknode_sdk::streams::{FilterLanguage, StreamDataset, StreamRegion, StreamStatus};

use crate::context::Ctx;
use crate::errors::CliError;

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
    /// Payload compression (`gzip` or `none`). Defaults to `none`.
    #[arg(long, default_value = "none")]
    pub compression: String,

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
        StreamCmd::List(a) => actions::list(a, ctx).await,
        StreamCmd::Create(a) => actions::create(*a, ctx).await,
        StreamCmd::Show { id } => actions::show(&id, ctx).await,
        StreamCmd::Update(a) => actions::update(a, ctx).await,
        StreamCmd::Delete { id } => actions::delete(&id, ctx).await,
        StreamCmd::DeleteAll => actions::delete_all(ctx).await,
        StreamCmd::Activate { id } => actions::activate(&id, ctx).await,
        StreamCmd::Pause { id } => actions::pause(&id, ctx).await,
        StreamCmd::TestFilter(a) => actions::test_filter(a, ctx).await,
        StreamCmd::EnabledCount { r#type } => actions::enabled_count(r#type, ctx).await,
    }
}
