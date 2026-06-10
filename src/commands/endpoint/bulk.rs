//! `qn endpoint bulk …` — bulk operations across many endpoints.
//!
//! Note on the SDK mapping: the SDK exposes a single
//! `bulk_update_endpoint_status(ids, status)` call where `status` is a free-form
//! string. We split this into two CLI verbs (`pause` / `resume`) to mirror the
//! single-endpoint `qn endpoint pause` / `qn endpoint resume` verbs. The mapping is:
//! `pause -> "paused"`, `resume -> "active"`.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    BulkAddTagRequest, BulkRemoveTagRequest, BulkUpdateEndpointStatusRequest,
};
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, OutputCtx, Render};

#[derive(Debug, Subcommand)]
pub enum BulkCmd {
    /// Pause many endpoints at once.
    Pause(IdsArgs),
    /// Resume (activate) many endpoints at once.
    Resume(IdsArgs),
    /// Manage tags on many endpoints at once.
    #[command(subcommand)]
    Tag(BulkTagCmd),
}

#[derive(Debug, ClapArgs)]
pub struct IdsArgs {
    /// Endpoint ids.
    pub ids: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum BulkTagCmd {
    /// Apply a tag to many endpoints (creates the tag if missing).
    Add(AddTagArgs),
    /// Remove a tag from many endpoints (by numeric tag id).
    Remove(RemoveTagArgs),
}

#[derive(Debug, ClapArgs)]
pub struct AddTagArgs {
    /// Tag label.
    #[arg(long)]
    pub label: String,
    /// Endpoint ids.
    pub ids: Vec<String>,
}

#[derive(Debug, ClapArgs)]
pub struct RemoveTagArgs {
    /// Tag id (numeric).
    #[arg(long)]
    pub tag_id: i32,
    /// Endpoint ids.
    pub ids: Vec<String>,
}

pub async fn run(cmd: BulkCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        BulkCmd::Pause(a) => set_status(a, "paused", ctx).await,
        BulkCmd::Resume(a) => set_status(a, "active", ctx).await,
        BulkCmd::Tag(BulkTagCmd::Add(a)) => tag_add(a, ctx).await,
        BulkCmd::Tag(BulkTagCmd::Remove(a)) => tag_remove(a, ctx).await,
    }
}

async fn set_status(a: IdsArgs, status: &str, ctx: Ctx) -> Result<(), CliError> {
    if a.ids.is_empty() {
        return Err(CliError::Arg("supply at least one endpoint id".to_string()));
    }
    // Pausing a fleet stops it serving requests — confirm with the blast
    // radius spelled out. Resuming restores service, so it stays unguarded.
    if status == "paused" {
        let cfg = ConfirmCfg::new(
            ctx.global.yes_count,
            ctx.global.no_input,
            ctx.out.stdout_is_tty,
        );
        let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
            true => true,
            false => prompt_yes_no(&format!(
                "Pause {} endpoint(s)? They will stop serving requests",
                a.ids.len()
            ))?,
        };
        if !proceed {
            return Err(CliError::Cancelled);
        }
    }
    let req = BulkUpdateEndpointStatusRequest {
        ids: a.ids,
        status: status.to_string(),
    };
    let resp = ctx.sdk.admin.bulk_update_endpoint_status(&req).await?;
    crate::output::emit(&ctx.out, &BulkStatusView(resp))
}

async fn tag_add(a: AddTagArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.ids.is_empty() {
        return Err(CliError::Arg("supply at least one endpoint id".to_string()));
    }
    let req = BulkAddTagRequest {
        ids: a.ids,
        label: a.label,
    };
    let resp = ctx.sdk.admin.bulk_add_tag(&req).await?;
    crate::output::emit(&ctx.out, &BulkAddTagView(resp))
}

async fn tag_remove(a: RemoveTagArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.ids.is_empty() {
        return Err(CliError::Arg("supply at least one endpoint id".to_string()));
    }
    let req = BulkRemoveTagRequest {
        ids: a.ids,
        tag_id: a.tag_id,
    };
    let resp = ctx.sdk.admin.bulk_remove_tag(&req).await?;
    crate::output::emit(&ctx.out, &BulkRemoveTagView(resp))
}

// ----- renderers ----- //

/// Shared body for all three bulk views: a summary line + an ID/OK table.
fn render_bulk_summary<'a, I>(
    w: &mut dyn std::io::Write,
    ctx: &OutputCtx,
    total: i32,
    updated_count: i32,
    failed_count: i32,
    results: I,
) -> std::io::Result<()>
where
    I: IntoIterator<Item = (&'a str, bool)>,
{
    writeln!(
        w,
        "total={total} updated={updated_count} failed={failed_count}"
    )?;
    let mut t = new_table(ctx);
    set_header_bold(&mut t, ctx, vec!["ID", "OK"]);
    for (id, success) in results {
        t.add_row(vec![
            Cell::new(id),
            Cell::new(if success { "✓" } else { "✗" }),
        ]);
    }
    write_table(w, &t)
}

#[derive(Serialize)]
struct BulkStatusView(quicknode_sdk::admin::BulkUpdateEndpointStatusResponse);

impl Render for BulkStatusView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let Some(data) = &self.0.data else {
            return writeln!(w, "(no result)");
        };
        render_bulk_summary(
            w,
            ctx,
            data.total,
            data.updated_count,
            data.failed_count,
            data.results.iter().map(|r| (r.id.as_str(), r.success)),
        )
    }
}

#[derive(Serialize)]
struct BulkAddTagView(quicknode_sdk::admin::BulkAddTagResponse);

impl Render for BulkAddTagView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let Some(data) = &self.0.data else {
            return writeln!(w, "(no result)");
        };
        render_bulk_summary(
            w,
            ctx,
            data.total,
            data.updated_count,
            data.failed_count,
            data.results.iter().map(|r| (r.id.as_str(), r.success)),
        )
    }
}

#[derive(Serialize)]
struct BulkRemoveTagView(quicknode_sdk::admin::BulkRemoveTagResponse);

impl Render for BulkRemoveTagView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let Some(data) = &self.0.data else {
            return writeln!(w, "(no result)");
        };
        render_bulk_summary(
            w,
            ctx,
            data.total,
            data.updated_count,
            data.failed_count,
            data.results.iter().map(|r| (r.id.as_str(), r.success)),
        )
    }
}
