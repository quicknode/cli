//! `qn endpoint tag …` — endpoint tag management.
//!
//! Covers both account-wide tag CRUD (`list`, `rename`, `delete`) and
//! per-endpoint tag operations (`add`, `remove`). Tags only exist to label
//! endpoints, which is why the account-wide CRUD lives here too.

use clap::Subcommand;
use comfy_table::Cell;
use quicknode_sdk::admin::{CreateTagRequest, RenameTagRequest};
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, Render};

#[derive(Debug, Subcommand)]
pub enum TagCmd {
    /// List every tag on the account with usage counts.
    #[command(visible_alias = "ls")]
    List,
    /// Rename a tag.
    Rename {
        /// Tag id (numeric).
        tag_id: i32,
        /// New label.
        label: String,
    },
    /// Delete a tag. The tag must not be applied to any endpoint.
    Delete {
        /// Tag id (numeric).
        tag_id: i32,
    },
    /// Tag an endpoint. Creates the tag on the account if missing.
    Add {
        /// Endpoint id.
        id: String,
        /// Tag label.
        label: String,
    },
    /// Remove a tag from an endpoint. `tag_id` is the numeric tag id from `qn endpoint tag list`.
    Remove {
        /// Endpoint id.
        id: String,
        /// Tag id (string).
        tag_id: String,
    },
}

pub async fn run(cmd: TagCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        TagCmd::List => list(ctx).await,
        TagCmd::Rename { tag_id, label } => rename(tag_id, label, ctx).await,
        TagCmd::Delete { tag_id } => delete(tag_id, ctx).await,
        TagCmd::Add { id, label } => add(id, label, ctx).await,
        TagCmd::Remove { id, tag_id } => remove(id, tag_id, ctx).await,
    }
}

async fn list(ctx: Ctx) -> Result<(), CliError> {
    let resp = crate::retry::retrying(ctx.global.retries, || ctx.sdk.admin.list_tags()).await?;
    crate::output::emit(&ctx.out, &TagsView(resp))
}

async fn rename(tag_id: i32, label: String, ctx: Ctx) -> Result<(), CliError> {
    let req = RenameTagRequest {
        label: label.clone(),
    };
    ctx.sdk.admin.rename_tag(tag_id, &req).await?;
    ctx.out.note(&format!("✓ Renamed tag {tag_id} → {label:?}"));
    Ok(())
}

async fn delete(tag_id: i32, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete tag {tag_id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.admin.delete_account_tag(tag_id).await?;
    ctx.out.note(&format!("✓ Deleted tag {tag_id}"));
    Ok(())
}

async fn add(id: String, label: String, ctx: Ctx) -> Result<(), CliError> {
    let req = CreateTagRequest {
        label: Some(label.clone()),
    };
    ctx.sdk.admin.create_tag(&id, &req).await?;
    ctx.out.note(&format!("✓ Tagged {id} with {label:?}"));
    Ok(())
}

async fn remove(id: String, tag_id: String, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.admin.delete_tag(&id, &tag_id).await?;
    ctx.out.note(&format!("✓ Removed tag {tag_id} from {id}"));
    Ok(())
}

#[derive(Serialize)]
struct TagsView(quicknode_sdk::admin::ListTagsResponse);

impl Render for TagsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no tag data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ID", "LABEL", "USAGE"]);
        for tg in &data.tags {
            t.add_row(vec![
                Cell::new(tg.id),
                Cell::new(&tg.label),
                Cell::new(tg.usage_count),
            ]);
        }
        write_table(w, &t)
    }
}
