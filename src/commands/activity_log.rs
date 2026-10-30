//! `qn activity-log …` — the account activity log: who changed what, when.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::{ActivityLogActor, ActivityLogTarget, ListActivityLogsRequest};
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{
    new_table, opt_cell, set_header_bold, write_pagination_footer, write_table, OutputCtx, Render,
};
use crate::retry::retrying;
use crate::time_arg;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn activity-log list\n  \
    qn activity-log list --type endpoint --endpoint-id 123 --from 7d\n  \
    qn activity-log filters\n  \
    qn activity-log show 7c9e6679-7425-40de-944b-e07fc1f90ae7")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: ActivityLogCmd,
}

#[derive(Debug, Subcommand)]
pub enum ActivityLogCmd {
    /// List account changes, newest first.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// List the actor ids and target types that `list` accepts as filters.
    Filters,
    /// Show one activity entry.
    Show {
        #[arg(value_name = "ACTIVITY_ID")]
        id: String,
    },
}

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn activity-log list --limit 50\n  \
    qn activity-log list --query prod --from 2026-01-01T00:00:00Z --to 2026-02-01T00:00:00Z\n  \
    qn activity-log list --actor 7c9e6679-7425-40de-944b-e07fc1f90ae7 --type endpoint\n  \
    qn activity-log list --to 2026-02-01T00:00:00Z --limit 100 --offset 100")]
pub struct ListArgs {
    /// Free-text search over descriptions, endpoint subdomains, and team names.
    #[arg(long, value_name = "TEXT")]
    pub query: Option<String>,
    /// Actor id (a user or API-key UUID from `qn activity-log filters`).
    #[arg(long, value_name = "ACTOR_ID")]
    pub actor: Option<String>,
    /// Only changes to this endpoint (numeric id).
    #[arg(long, value_name = "NUMERIC_ID")]
    pub endpoint_id: Option<String>,
    /// Target type, e.g. `endpoint`, `team`, `api_key` (see `qn activity-log filters`).
    #[arg(long = "type", value_name = "TARGET_TYPE")]
    pub target_type: Option<String>,
    /// Start of the window (RFC-3339, a relative duration like `7d`, or `now`).
    #[arg(long)]
    pub from: Option<String>,
    /// End of the window. Pin it when paging so new entries don't shift pages.
    #[arg(long)]
    pub to: Option<String>,
    /// Page size (0-100; the API defaults to 20).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    pub limit: Option<i32>,
    /// Entries to skip.
    #[arg(long)]
    pub offset: Option<i64>,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        ActivityLogCmd::List(a) => list(a, ctx).await,
        ActivityLogCmd::Filters => filters(ctx).await,
        ActivityLogCmd::Show { id } => show(&id, ctx).await,
    }
}

async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let req = ListActivityLogsRequest {
        q: a.query,
        actor: a.actor,
        endpoint_id: a.endpoint_id,
        target_type: a.target_type,
        from_time: a.from.as_deref().map(time_arg::parse_rfc3339).transpose()?,
        to_time: a.to.as_deref().map(time_arg::parse_rfc3339).transpose()?,
        limit: a.limit,
        offset: a.offset,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.list_activity_logs(&req)
    })
    .await?;
    crate::output::emit(&ctx.out, &ActivityLogsView(resp))
}

async fn filters(ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.list_activity_log_filters()
    })
    .await?;
    crate::output::emit(&ctx.out, &FiltersView(resp))
}

async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_activity_log(id)).await?;
    crate::output::emit(&ctx.out, &ActivityLogView(resp))
}

// ----- renderers ----- //

/// `kind:label`, or just the label when the API gave no kind.
fn target_text(t: &ActivityLogTarget) -> String {
    match &t.kind {
        Some(kind) => format!("{kind}:{}", t.label),
        None => t.label.clone(),
    }
}

/// The most recognisable handle for an actor: name, then email, then its
/// type (which is all a deleted user or Quicknode itself has).
fn actor_text(a: &ActivityLogActor) -> &str {
    a.name
        .as_deref()
        .or(a.email.as_deref())
        .unwrap_or(&a.actor_type)
}

#[derive(Serialize)]
struct ActivityLogsView(quicknode_sdk::admin::ListActivityLogsResponse);

impl Render for ActivityLogsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "TIME", "ACTION", "TARGET", "ACTOR", "DESCRIPTION"],
        );
        for e in &self.0.data {
            t.add_row(vec![
                Cell::new(&e.id),
                Cell::new(&e.occurred_at),
                Cell::new(&e.action),
                Cell::new(target_text(&e.target)),
                Cell::new(actor_text(&e.actor)),
                opt_cell(&e.description),
            ]);
        }
        write_table(w, &t)?;
        let p = &self.0.pagination;
        write_pagination_footer(w, p.offset, self.0.data.len(), p.total)
    }
}

#[derive(Serialize)]
struct FiltersView(quicknode_sdk::admin::ListActivityLogFiltersResponse);

impl Render for FiltersView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let d = &self.0.data;
        writeln!(w, "ACTORS ({})", d.actors.len())?;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ID", "NAME", "TYPE"]);
        for a in &d.actors {
            t.add_row(vec![
                Cell::new(&a.id),
                opt_cell(&a.name),
                Cell::new(&a.actor_type),
            ]);
        }
        write_table(w, &t)?;
        writeln!(w)?;
        writeln!(w, "TYPES ({})", d.types.len())?;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["VALUE", "LABEL"]);
        for ty in &d.types {
            t.add_row(vec![Cell::new(&ty.value), Cell::new(&ty.label)]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct ActivityLogView(quicknode_sdk::admin::GetActivityLogResponse);

impl Render for ActivityLogView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let Some(e) = &self.0.data else {
            return writeln!(w, "(no activity entry)");
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(&e.id)]);
        t.add_row(vec![Cell::new("occurred_at"), Cell::new(&e.occurred_at)]);
        t.add_row(vec![Cell::new("action"), Cell::new(&e.action)]);
        t.add_row(vec![Cell::new("description"), opt_cell(&e.description)]);
        t.add_row(vec![Cell::new("target.id"), Cell::new(&e.target.id)]);
        t.add_row(vec![Cell::new("target.kind"), opt_cell(&e.target.kind)]);
        t.add_row(vec![Cell::new("target.label"), Cell::new(&e.target.label)]);
        t.add_row(vec![Cell::new("target.name"), opt_cell(&e.target.name)]);
        t.add_row(vec![Cell::new("actor.id"), opt_cell(&e.actor.id)]);
        t.add_row(vec![
            Cell::new("actor.type"),
            Cell::new(&e.actor.actor_type),
        ]);
        t.add_row(vec![Cell::new("actor.name"), opt_cell(&e.actor.name)]);
        t.add_row(vec![Cell::new("actor.email"), opt_cell(&e.actor.email)]);
        write_table(w, &t)
    }
}
