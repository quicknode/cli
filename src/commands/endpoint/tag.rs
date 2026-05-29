//! `qn endpoint tag {add,remove}` — per-endpoint tag management.
//!
//! For account-wide tag list/rename/delete, see `qn tag`.

use clap::Subcommand;
use quicknode_sdk::admin::CreateTagRequest;

use crate::context::Ctx;
use crate::errors::CliError;

#[derive(Debug, Subcommand)]
pub enum TagCmd {
    /// Tag an endpoint. Creates the tag on the account if missing.
    Add {
        /// Endpoint id.
        id: String,
        /// Tag label.
        label: String,
    },
    /// Remove a tag from an endpoint. `tag_id` is the numeric tag id from `qn tag list`.
    Remove {
        /// Endpoint id.
        id: String,
        /// Tag id (string).
        tag_id: String,
    },
}

pub async fn run(cmd: TagCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        TagCmd::Add { id, label } => {
            let req = CreateTagRequest {
                label: Some(label.clone()),
            };
            ctx.sdk.admin.create_tag(&id, &req).await?;
            ctx.out.note(&format!("✓ Tagged {id} with {label:?}"));
        }
        TagCmd::Remove { id, tag_id } => {
            ctx.sdk.admin.delete_tag(&id, &tag_id).await?;
            ctx.out.note(&format!("✓ Removed tag {tag_id} from {id}"));
        }
    }
    Ok(())
}
