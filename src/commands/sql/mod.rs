//! `qn sql …` — run SQL queries and inspect cluster schemas.

mod render;

use std::io::Read;
use std::path::PathBuf;

use clap::{ArgGroup, Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::errors::SdkError;
use quicknode_sdk::{ChainSchema, QueryParams, QueryResponse, SqlCluster};
use serde::Serialize;
use serde_json::Value;

use crate::commands::rpc::mpp::PayScope;
use crate::commands::rpc::payment::{
    ensure_gateway_session, is_token_expired, reauthenticate, resolve_payment_params,
    resolve_session_params, PaymentParams, SessionParams,
};
use crate::config::{self, PaymentSection};
use crate::context::{Ctx, GlobalArgs};
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, Format, Render};
use crate::retry::retrying;
use render::json_cell;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn sql clusters\n  \
    qn sql schema hyperliquid-core-mainnet\n  \
    qn sql query \"SELECT 1\" --cluster-id hyperliquid-core-mainnet\n  \
    qn sql query \"SELECT * FROM hyperliquid_trades LIMIT 10\" \\\n      \
    --cluster-id hyperliquid-core-mainnet \\\n      \
    --x402-drawdown --payment-wallet payer --payment-network base-sepolia")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: SqlCmd,
}

#[derive(Debug, Subcommand)]
pub enum SqlCmd {
    /// List clusters in the public SQL catalog. No API key required.
    #[command(visible_alias = "ls")]
    #[command(after_help = "Examples:\n  \
        qn sql clusters\n  \
        qn sql ls -o json")]
    Clusters,

    /// Run a read-only SQL query against a cluster.
    ///
    /// The query may be passed inline, read from a file with --file, or read
    /// from stdin with `--file -`. Results are capped at 1000 rows per request;
    /// page through larger result sets with LIMIT/OFFSET in the SQL.
    ///
    /// Only this verb chooses who pays. Discovery (`clusters`, `schema`) is
    /// always free. Config never turns a payment path on.
    #[command(after_help = "Examples:\n  \
        qn sql query \"SELECT 1\" --cluster-id hyperliquid-core-mainnet\n  \
        qn sql query --file query.sql --cluster-id hyperliquid-core-mainnet\n  \
        cat query.sql | qn sql query --file - --cluster-id hyperliquid-core-mainnet\n  \
        qn sql query \"SELECT * FROM hyperliquid_trades LIMIT 10\" \\\n      \
        --cluster-id hyperliquid-core-mainnet \\\n      \
        --x402-drawdown --payment-wallet payer --payment-network base-sepolia\n  \
        qn sql query \"SELECT * FROM hyperliquid_trades LIMIT 10\" \\\n      \
        --cluster-id hyperliquid-core-mainnet \\\n      \
        --mpp-session --payment-wallet payer \\\n      \
        --payment-network tempo-testnet --payment-asset pathUSD --max-amount 1000000")]
    Query(Box<QueryArgs>),

    /// Show a cluster's table schema (tables, engines, columns, types).
    /// No API key required; always hits the public catalog.
    #[command(after_help = "Examples:\n  \
        qn sql schema hyperliquid-core-mainnet")]
    Schema(SchemaArgs),
}

#[derive(Debug, ClapArgs)]
#[command(
    group(ArgGroup::new("source").args(["query", "file"]).required(true)),
    group(ArgGroup::new("payment").args(["x402_drawdown", "mpp_session"])),
)]
pub struct QueryArgs {
    /// The SQL query to run. Mutually exclusive with --file.
    #[arg(value_name = "SQL")]
    pub query: Option<String>,

    /// Read the query from a file, or from stdin when the path is `-`.
    #[arg(long, short = 'f', value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// The cluster to query (e.g. hyperliquid-core-mainnet).
    #[arg(long, value_name = "CLUSTER_ID")]
    pub cluster_id: String,

    /// Pay from prepaid x402 credits (SIWX session). Not a per-request
    /// payment: buy credits first with `qn micropayments x402 buy-credits`.
    /// An API key, if also present, is unused.
    #[arg(long, help_heading = "Payment")]
    pub x402_drawdown: bool,

    /// Pay from an open MPP channel (session voucher). Open a channel first
    /// with `qn micropayments mpp open`. An API key, if also present, is unused.
    #[arg(long, help_heading = "Payment")]
    pub mpp_session: bool,

    /// File containing the raw payment private key; pass `-` to read it from
    /// stdin. Precedence: this flag > --payment-wallet > `key_file` > `wallet`
    /// under [rpc.payment] in config.
    #[arg(
        long,
        value_name = "PATH",
        requires = "payment",
        conflicts_with = "payment_wallet",
        help_heading = "Payment"
    )]
    pub payment_key_file: Option<PathBuf>,

    /// Name of a stored wallet (from `qn wallet generate`) to pay with.
    #[arg(
        long,
        value_name = "NAME",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub payment_wallet: Option<String>,

    /// Chain you PAY on — a network name or CAIP-2 id. Falls back to
    /// `payment_network` under [rpc.payment].
    #[arg(
        long,
        value_name = "NETWORK",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub payment_network: Option<String>,

    /// Token to pay with. Required for `--mpp-session`. Falls back to
    /// `payment_asset` under [rpc.payment].
    #[arg(
        long,
        value_name = "ADDRESS",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub payment_asset: Option<String>,

    /// Spend ceiling in integer base units of the asset. Required for
    /// `--mpp-session`. Falls back to `max_amount` under [rpc.payment].
    #[arg(
        long,
        value_name = "BASE_UNITS",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub max_amount: Option<String>,

    /// Explicit Solana RPC URL for x402/Solana session auth. Falls back to
    /// `svm_rpc_url` in [rpc.payment], then a public Solana RPC.
    #[arg(
        long,
        value_name = "URL",
        requires = "payment",
        help_heading = "Payment"
    )]
    pub svm_rpc_url: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct SchemaArgs {
    /// The cluster whose schema to show.
    #[arg(value_name = "CLUSTER_ID")]
    pub cluster_id: String,
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        SqlCmd::Clusters => clusters(global).await,
        SqlCmd::Query(a) => query(*a, global).await,
        SqlCmd::Schema(a) => schema(a, global).await,
    }
}

async fn clusters(global: GlobalArgs) -> Result<(), CliError> {
    let ctx = Ctx::from_global_keyless_sql_catalog(global)?;
    let resp = retrying(ctx.global.retries, || ctx.sdk.sql.list_clusters()).await?;
    crate::output::emit(&ctx.out, &ClustersView(resp))
}

async fn schema(a: SchemaArgs, global: GlobalArgs) -> Result<(), CliError> {
    let ctx = Ctx::from_global_keyless_sql_catalog(global)?;
    let resp = retrying(ctx.global.retries, || ctx.sdk.sql.get_schema(&a.cluster_id)).await?;
    crate::output::emit(&ctx.out, &SchemaView(resp))
}

async fn query(a: QueryArgs, global: GlobalArgs) -> Result<(), CliError> {
    let sql = resolve_query(a.query.clone(), a.file.clone())?;
    let params = QueryParams {
        query: sql,
        cluster_id: a.cluster_id.clone(),
    };
    if a.x402_drawdown {
        return query_x402_drawdown(a, params, global).await;
    }
    if a.mpp_session {
        return query_mpp_session(a, params, global).await;
    }
    let ctx = match Ctx::from_global(global) {
        Ok(ctx) => ctx,
        Err(CliError::NoApiKey) => return Err(CliError::Arg(no_payer_message())),
        Err(e) => return Err(e),
    };
    // A query consumes credits and may be expensive; never retry, a retried
    // query re-runs and re-bills.
    let resp = ctx.sdk.sql.query(&params).await?;
    emit_query(&ctx, resp)
}

fn emit_query(ctx: &Ctx, resp: QueryResponse) -> Result<(), CliError> {
    // Stats are diagnostics: they go to stderr (suppressed by --quiet) so stdout
    // stays clean for piping. JSON/YAML/TOON already carry the full response, so
    // only emit the note for the human-facing table/markdown formats.
    if matches!(ctx.out.format, Format::Table | Format::Md) {
        ctx.out.note(&stats_line(&resp));
    }
    crate::output::emit(&ctx.out, &QueryView(resp))
}

fn no_payer_message() -> String {
    "sql query needs a payment method or an API key.\n  \
     Pay with prepaid x402 credits:\n    \
     qn sql query \"SELECT 1\" --cluster-id hyperliquid-core-mainnet \\\n      \
     --x402-drawdown --payment-wallet <NAME> --payment-network base-sepolia\n  \
     Pay from an MPP channel:\n    \
     qn sql query \"SELECT 1\" --cluster-id hyperliquid-core-mainnet \\\n      \
     --mpp-session --payment-wallet <NAME> \\\n      \
     --payment-network tempo-testnet --payment-asset pathUSD --max-amount 1000000\n  \
     Or set an API key: qn auth login"
        .to_string()
}

fn load_payment_section(global: &GlobalArgs) -> Result<PaymentSection, CliError> {
    let Some(path) = global.resolve_config_path() else {
        return Ok(PaymentSection::default());
    };
    Ok(config::load_from(&path)?
        .map(|cfg| cfg.rpc.payment)
        .unwrap_or_default())
}

async fn query_x402_drawdown(
    a: QueryArgs,
    params: QueryParams,
    global: GlobalArgs,
) -> Result<(), CliError> {
    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_session_params(
        &SessionParams {
            key_file: a.payment_key_file.as_deref(),
            wallet: a.payment_wallet.as_deref(),
            payment_network: a.payment_network.as_deref(),
            svm_rpc_url: a.svm_rpc_url.as_deref(),
        },
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;
    let ctx = Ctx::from_global_keyless_sql_payment(global.clone(), payment)?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }

    let session = ensure_gateway_session(&ctx, &global).await?;
    let resp = match ctx.sdk.sql.query_with_session(&params, &session).await {
        Ok(resp) => resp,
        Err(e) if is_token_expired(&e) => {
            let fresh = reauthenticate(&ctx, &global).await?;
            match ctx.sdk.sql.query_with_session(&params, &fresh).await {
                Ok(resp) => resp,
                Err(e) => return Err(map_sql_drawdown_error(e)),
            }
        }
        Err(e) => return Err(map_sql_drawdown_error(e)),
    };
    emit_query(&ctx, resp)
}

fn map_sql_drawdown_error(e: SdkError) -> CliError {
    if is_sql_requires_payment(&e) {
        return CliError::PaymentRefused(
            "out of x402 credits. Buy more with \
             'qn micropayments x402 buy-credits', then retry this query."
                .to_string(),
        );
    }
    e.into()
}

fn is_sql_requires_payment(e: &SdkError) -> bool {
    matches!(
        e,
        SdkError::Api { status, body }
            if status.as_u16() == 402
                || body.contains("requires_payment")
                || body.contains("insufficient_credits")
                || body.contains("no_credits")
    )
}

async fn query_mpp_session(
    a: QueryArgs,
    params: QueryParams,
    global: GlobalArgs,
) -> Result<(), CliError> {
    let section = load_payment_section(&global)?;
    let wallets_dir = config::wallets_dir(global.resolve_config_path().as_deref());
    let (payment, key_file_warning) = resolve_payment_params(
        "mpp",
        &PaymentParams {
            key_file: a.payment_key_file.as_deref(),
            wallet: a.payment_wallet.as_deref(),
            max_amount: a.max_amount.as_deref(),
            payment_network: a.payment_network.as_deref(),
            payment_asset: a.payment_asset.as_deref(),
            svm_rpc_url: a.svm_rpc_url.as_deref(),
        },
        &section,
        wallets_dir.as_deref(),
        global.base_url.clone(),
    )?;
    let pay_scope = PayScope::from_config(&payment);
    let ctx = Ctx::from_global_keyless_sql_payment(global.clone(), payment.clone())?;
    if let Some(w) = key_file_warning {
        ctx.out.warn(&w);
    }

    let address = ctx.sdk.rpc.payment_address()?;
    let scope = pay_scope.with_address(address);
    let channels_path = config::channels_cache_path(global.resolve_config_path().as_deref());
    let mut channel = channels_path
        .as_deref()
        .and_then(|p| config::load_channel(p, &scope))
        .ok_or_else(|| {
            CliError::Arg(format!(
                "no open MPP channel for this wallet paying {}. Open one with \
                 'qn micropayments mpp open --deposit <BASE_UNITS>'.",
                pay_scope.describe()
            ))
        })?;

    let result = match ctx
        .sdk
        .sql
        .query_with_mpp_session(&params, &payment, &channel)
        .await
    {
        Ok(result) => result,
        Err(e) => return Err(map_sql_mpp_error(e)),
    };

    channel.cumulative_spent = result.accepted_cumulative;
    if let Some(path) = &channels_path {
        let _ = config::save_channel(path, &scope, &channel);
    }
    emit_query(&ctx, result.query)
}

fn map_sql_mpp_error(e: SdkError) -> CliError {
    if let SdkError::Api { status, body } = &e {
        if status.as_u16() == 402
            || body.contains("amount-exceeds-deposit")
            || body.contains("AmountExceedsDeposit")
            || body.contains("insufficient")
        {
            return CliError::PaymentRefused(
                "the MPP channel can't cover this query. Top up with \
                 'qn micropayments mpp top-up', or open a new channel with \
                 'qn micropayments mpp open'."
                    .to_string(),
            );
        }
    }
    if let SdkError::PaymentUnsupported { offered } = &e {
        if offered.contains("exceeds channel deposit") || offered.contains("top up") {
            return CliError::PaymentRefused(
                "the MPP channel can't cover this query. Top up with \
                 'qn micropayments mpp top-up', or open a new channel with \
                 'qn micropayments mpp open'."
                    .to_string(),
            );
        }
    }
    e.into()
}

/// Resolves the query text from the inline arg, a file, or stdin (`-`). Exactly
/// one of `query`/`file` is guaranteed by the clap `ArgGroup`.
fn resolve_query(query: Option<String>, file: Option<PathBuf>) -> Result<String, CliError> {
    if let Some(q) = query {
        return Ok(q);
    }
    let path = file.expect("clap ArgGroup guarantees one of query/file");
    if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::Arg(format!("could not read query from stdin: {e}")))?;
        return Ok(buf);
    }
    std::fs::read_to_string(&path).map_err(|e| {
        CliError::Arg(format!(
            "could not read query file '{}': {e}",
            path.display()
        ))
    })
}

/// Builds the human-facing stats note, e.g.
/// `✓ 2 rows · 135 credits · 0.007s` plus a truncation hint when the result set
/// was capped below the total matched.
fn stats_line(resp: &QueryResponse) -> String {
    let mut line = format!(
        "✓ {} rows · {} credits · {:.3}s",
        resp.rows, resp.credits, resp.statistics.elapsed
    );
    if resp.rows_before_limit_at_least > resp.rows {
        line.push_str(&format!(
            " · {} matched (use LIMIT/OFFSET to page)",
            resp.rows_before_limit_at_least
        ));
    }
    line
}

#[derive(Serialize)]
struct ClustersView(Vec<SqlCluster>);

impl Render for ClustersView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, ["ID", "DISPLAY_NAME"]);
        for cluster in &self.0 {
            t.add_row(vec![
                Cell::new(&cluster.id),
                Cell::new(&cluster.display_name),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct QueryView(QueryResponse);

impl Render for QueryView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        // Headers come from `meta` (ordered, authoritative) so an empty result
        // set still prints its column headers.
        set_header_bold(
            &mut t,
            ctx,
            self.0.meta.iter().map(|c| c.name.to_uppercase()),
        );
        for row in &self.0.data {
            let cells = self.0.meta.iter().map(|col| {
                let v = row.get(&col.name).unwrap_or(&Value::Null);
                Cell::new(json_cell(v))
            });
            t.add_row(cells);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct SchemaView(ChainSchema);

impl Render for SchemaView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let s = &self.0;
        let n = s.tables.len();
        writeln!(
            w,
            "{} · {} · {} table{}",
            s.chain,
            s.cluster_id,
            n,
            if n == 1 { "" } else { "s" }
        )?;
        for table in &s.tables {
            writeln!(w)?;
            writeln!(
                w,
                "{} ({}, {} rows)",
                table.name, table.engine, table.total_rows
            )?;
            let partition = if table.partition_key.is_empty() {
                "—".to_string()
            } else {
                table.partition_key.clone()
            };
            let sorting = if table.sorting_key.is_empty() {
                "—".to_string()
            } else {
                table.sorting_key.join(", ")
            };
            writeln!(w, "  partition: {partition}")?;
            writeln!(w, "  sorting: {sorting}")?;
            let mut t = new_table(ctx);
            set_header_bold(&mut t, ctx, vec!["COLUMN", "TYPE"]);
            for col in &table.columns {
                t.add_row(vec![Cell::new(&col.name), Cell::new(&col.column_type)]);
            }
            write_table(w, &t)?;
        }
        Ok(())
    }
}
