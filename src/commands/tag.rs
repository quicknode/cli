//! `qn tag …` — account-wide tag management.

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::RenameTagRequest;
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, Render};

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: TagCmd,
}

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
        id: i32,
    },
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        TagCmd::List => list(ctx).await,
        TagCmd::Rename { tag_id, label } => rename(tag_id, label, ctx).await,
        TagCmd::Delete { id } => delete(id, ctx).await,
    }
}

async fn list(ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.list_tags().await?;
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

async fn delete(id: i32, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete tag {id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.admin.delete_account_tag(id).await?;
    ctx.out.note(&format!("✓ Deleted tag {id}"));
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
