//! `qn transaction …` — retained transactions submitted through the
//! account's endpoints, with their submission attempts.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::{GetTransactionRequest, ListTransactionsRequest};
use serde::Serialize;

use super::summary::{non_empty, write_cursor_footer, WindowArgs};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{
    bool_cell, new_table, opt_cell, set_header_bold, write_table, OutputCtx, Render,
};
use crate::retry::retrying;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn transaction list --from 1h\n  \
    qn transaction list --from 24h --state failed --network ethereum-mainnet\n  \
    qn transaction show ethereum-mainnet 0xabc --payloads")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: TransactionCmd,
}

#[derive(Debug, Subcommand)]
pub enum TransactionCmd {
    /// List retained transactions in a time window, newest first.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Show a transaction's retained details and submission attempts.
    Show(ShowArgs),
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn transaction list --from 1h\n  \
    qn transaction list --from 24h --state failed --endpoint 123 --limit 50\n  \
    qn transaction list --from 7d --method eth_sendRawTransaction --hash 0xabc\n  \
    qn transaction list --cursor eyJvZmZzZXQiOjEwMH0")]
pub struct ListArgs {
    #[command(flatten)]
    pub window: WindowArgs,
    /// Only this network (e.g. `ethereum-mainnet`).
    #[arg(long)]
    pub network: Option<String>,
    /// Only transactions submitted with this RPC method.
    #[arg(long)]
    pub method: Option<String>,
    /// Only this state.
    #[arg(long, value_parser = ["submitted", "succeeded", "failed"])]
    pub state: Option<String>,
    /// Only this transaction hash.
    #[arg(long, value_name = "HASH")]
    pub hash: Option<String>,
    /// Only these submitting endpoints (numeric ids; repeatable).
    #[arg(long = "endpoint", value_name = "NUMERIC_ID")]
    pub endpoints: Vec<i64>,
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn transaction show ethereum-mainnet 0xabc\n  \
    qn transaction show ethereum-mainnet 0xabc --payloads -o json")]
pub struct ShowArgs {
    /// Network the transaction was submitted on (e.g. `ethereum-mainnet`).
    #[arg(value_name = "NETWORK")]
    pub network: String,
    /// Transaction hash.
    #[arg(value_name = "HASH")]
    pub hash: String,
    /// Include each attempt's retained request and response text.
    #[arg(long)]
    pub payloads: bool,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        TransactionCmd::List(a) => list(a, ctx).await,
        TransactionCmd::Show(a) => show(a, ctx).await,
    }
}

async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let filters_given = a.network.is_some()
        || a.method.is_some()
        || a.state.is_some()
        || a.hash.is_some()
        || !a.endpoints.is_empty();
    let w = a.window.resolve(filters_given)?;
    let req = ListTransactionsRequest {
        last: None,
        from_time: w.from_time,
        to_time: w.to_time,
        network: a.network,
        method: a.method,
        state: a.state,
        transaction_hash: a.hash,
        cursor: w.cursor,
        endpoints: non_empty(a.endpoints),
        limit: w.limit,
    };
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.list_transactions(&req)).await?;
    crate::output::emit(&ctx.out, &TransactionsView(resp))
}

async fn show(a: ShowArgs, ctx: Ctx) -> Result<(), CliError> {
    // Payloads are opt-in on the API too; only send the flag when asked for.
    let req = GetTransactionRequest {
        include_payloads: a.payloads.then_some(true),
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_transaction(&a.network, &a.hash, &req)
    })
    .await?;
    crate::output::emit(&ctx.out, &TransactionView(resp))
}

// ----- renderers ----- //

/// Endpoint ids as a comma list; a null entry (an endpoint the API could not
/// resolve) renders as `—`.
fn endpoint_ids_text(ids: &[Option<i64>]) -> String {
    ids.iter()
        .map(|id| match id {
            Some(id) => id.to_string(),
            None => "—".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Serialize)]
struct TransactionsView(quicknode_sdk::admin::ListTransactionsResponse);

impl Render for TransactionsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let d = &self.0.data;
        writeln!(w, "window: {} → {}", d.from_time, d.to_time)?;
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec![
                "NETWORK",
                "HASH",
                "STATE",
                "ATTEMPTS",
                "ENDPOINTS",
                "METHODS",
                "HEIGHT",
                "LAST_SUBMITTED",
            ],
        );
        for tx in &d.transactions {
            t.add_row(vec![
                Cell::new(&tx.network),
                Cell::new(&tx.transaction_hash),
                Cell::new(&tx.state),
                Cell::new(tx.attempt_count),
                Cell::new(endpoint_ids_text(&tx.endpoint_ids)),
                Cell::new(tx.methods.join(", ")),
                opt_cell(&tx.height),
                Cell::new(&tx.last_submitted_at),
            ]);
        }
        write_table(w, &t)?;
        write_cursor_footer(w, d.next_cursor.as_deref())
    }
}

#[derive(Serialize)]
struct TransactionView(quicknode_sdk::admin::GetTransactionResponse);

impl Render for TransactionView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let lookup = &self.0.data;
        let Some(d) = lookup.data.as_ref().filter(|_| lookup.exists) else {
            writeln!(w, "exists: {}", lookup.exists)?;
            return writeln!(w, "(no retained details for this transaction)");
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("network"), Cell::new(&d.network)]);
        t.add_row(vec![Cell::new("hash"), Cell::new(&d.transaction_hash)]);
        t.add_row(vec![Cell::new("state"), Cell::new(&d.state)]);
        t.add_row(vec![
            Cell::new("first_submitted_at"),
            Cell::new(&d.first_submitted_at),
        ]);
        t.add_row(vec![
            Cell::new("last_submitted_at"),
            Cell::new(&d.last_submitted_at),
        ]);
        t.add_row(vec![Cell::new("attempt_count"), opt_cell(&d.attempt_count)]);
        t.add_row(vec![
            Cell::new("endpoint_ids"),
            opt_cell(&d.endpoint_ids.as_deref().map(endpoint_ids_text)),
        ]);
        t.add_row(vec![
            Cell::new("methods"),
            opt_cell(&d.methods.as_ref().map(|m| m.join(", "))),
        ]);
        t.add_row(vec![Cell::new("archive"), bool_cell(d.archive)]);
        t.add_row(vec![Cell::new("height"), opt_cell(&d.height)]);
        write_table(w, &t)?;

        writeln!(w)?;
        writeln!(w, "ATTEMPTS ({})", d.attempts.len())?;
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["SUBMITTED_AT", "ENDPOINT_ID", "METHOD", "REQUEST_ID"],
        );
        for a in &d.attempts {
            t.add_row(vec![
                Cell::new(&a.submitted_at),
                Cell::new(a.endpoint_id),
                Cell::new(&a.method),
                Cell::new(&a.request_id),
            ]);
        }
        write_table(w, &t)?;

        // Payload text is printed verbatim: the API retains it unparsed and
        // it may not be JSON.
        for (i, a) in d.attempts.iter().enumerate() {
            let Some(p) = &a.payloads else { continue };
            writeln!(w)?;
            writeln!(w, "== attempt {} request ==", i + 1)?;
            writeln!(w, "{}", p.request.as_deref().unwrap_or("(none)"))?;
            writeln!(w, "== attempt {} response ==", i + 1)?;
            writeln!(w, "{}", p.response.as_deref().unwrap_or("(none)"))?;
        }
        Ok(())
    }
}
