//! `qn billing …` — invoices and payments.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: BillingCmd,
}

#[derive(Debug, Subcommand)]
pub enum BillingCmd {
    /// List invoices on the account.
    Invoices,
    /// List payments on the account.
    Payments,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        BillingCmd::Invoices => invoices(ctx).await,
        BillingCmd::Payments => payments(ctx).await,
    }
}

async fn invoices(ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.list_invoices().await?;
    crate::output::emit(&ctx.out, &InvoicesView(resp))
}

async fn payments(ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.list_payments().await?;
    crate::output::emit(&ctx.out, &PaymentsView(resp))
}

#[derive(Serialize)]
struct InvoicesView(quicknode_sdk::admin::ListInvoicesResponse);

impl Render for InvoicesView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no invoices)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec![
                "ID",
                "STATUS",
                "REASON",
                "AMOUNT_DUE",
                "AMOUNT_PAID",
                "CREATED",
            ],
        );
        for i in &data.invoices {
            t.add_row(vec![
                Cell::new(&i.id),
                Cell::new(&i.status),
                Cell::new(&i.billing_reason),
                Cell::new(i.amount_due),
                Cell::new(i.amount_paid),
                Cell::new(i.created),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct PaymentsView(quicknode_sdk::admin::ListPaymentsResponse);

impl Render for PaymentsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no payments)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["CREATED", "AMOUNT", "CURRENCY", "CARD"]);
        for p in &data.payments {
            t.add_row(vec![
                Cell::new(&p.created_at),
                Cell::new(&p.amount),
                Cell::new(&p.currency),
                opt_cell(&p.card_last_4),
            ]);
        }
        write_table(w, &t)
    }
}
