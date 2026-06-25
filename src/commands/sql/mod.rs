//! `qn sql …` — run SQL queries and inspect cluster schemas.

mod render;

use std::io::Read;
use std::path::PathBuf;

use clap::{ArgGroup, Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::{ChainSchema, QueryParams, QueryResponse};
use serde::Serialize;
use serde_json::Value;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, Format, Render};
use crate::retry::retrying;
use render::json_cell;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: SqlCmd,
}

#[derive(Debug, Subcommand)]
pub enum SqlCmd {
    /// Run a read-only SQL query against a cluster.
    ///
    /// The query may be passed inline, read from a file with --file, or read
    /// from stdin with `--file -`. Results are capped at 1000 rows per request;
    /// page through larger result sets with LIMIT/OFFSET in the SQL.
    #[command(after_help = "Examples:\n  \
        qn sql query \"SELECT 1\" --cluster-id hyperliquid-core-mainnet\n  \
        qn sql query --file query.sql --cluster-id hyperliquid-core-mainnet\n  \
        cat query.sql | qn sql query --file - --cluster-id hyperliquid-core-mainnet")]
    Query(QueryArgs),

    /// Show a cluster's table schema (tables, engines, columns, types).
    Schema(SchemaArgs),
}

#[derive(Debug, ClapArgs)]
#[command(group(ArgGroup::new("source").args(["query", "file"]).required(true)))]
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
}

#[derive(Debug, ClapArgs)]
pub struct SchemaArgs {
    /// The cluster whose schema to show.
    #[arg(value_name = "CLUSTER_ID")]
    pub cluster_id: String,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        SqlCmd::Query(a) => query(a, ctx).await,
        SqlCmd::Schema(a) => schema(a, ctx).await,
    }
}

async fn query(a: QueryArgs, ctx: Ctx) -> Result<(), CliError> {
    let sql = resolve_query(a.query, a.file)?;
    let params = QueryParams {
        query: sql,
        cluster_id: a.cluster_id,
    };
    // A query consumes credits and may be expensive; never retry, a retried
    // query re-runs and re-bills.
    let resp = ctx.sdk.sql.query(&params).await?;

    // Stats are diagnostics: they go to stderr (suppressed by --quiet) so stdout
    // stays clean for piping. JSON/YAML/TOON already carry the full response, so
    // only emit the note for the human-facing table/markdown formats.
    if matches!(ctx.out.format, Format::Table | Format::Md) {
        ctx.out.note(&stats_line(&resp));
    }
    crate::output::emit(&ctx.out, &QueryView(resp))
}

async fn schema(a: SchemaArgs, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.sql.get_schema(&a.cluster_id)).await?;
    crate::output::emit(&ctx.out, &SchemaView(resp))
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
