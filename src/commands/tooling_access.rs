//! `qn tooling-access {status,enable,disable}` — manage Tooling Access.
//!
//! Tooling Access provisions a single multichain, read-only endpoint for the
//! account and is the prerequisite for `qn rpc`. `enable`/`disable` require an
//! admin role; `status` works for any role. These map to the admin control
//! plane (`qn.admin.{tooling_access_status,enable,disable}`).

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_table, Render};
use crate::retry::retrying;

#[derive(Debug, ClapArgs)]
#[command(after_help = "Examples:\n  \
    qn tooling-access status\n  \
    qn tooling-access enable        # provisions the endpoint (admin role)\n  \
    qn tooling-access disable")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: ToolingAccessCmd,
}

#[derive(Debug, Subcommand)]
pub enum ToolingAccessCmd {
    /// Show whether Tooling Access is enabled and the endpoint URL.
    Status,
    /// Enable (provision) Tooling Access. Idempotent; requires an admin role.
    Enable,
    /// Disable Tooling Access, pausing the endpoint. Idempotent.
    Disable,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        ToolingAccessCmd::Status => {
            // status is read-only, so retry on transient failures.
            let resp = retrying(ctx.global.retries, || {
                ctx.sdk.admin.tooling_access_status()
            })
            .await?;
            crate::output::emit(&ctx.out, &StatusView(resp))
        }
        // enable/disable mutate account state — never retry.
        ToolingAccessCmd::Enable => {
            let resp = ctx.sdk.admin.enable_tooling_access().await?;
            ctx.out.note("✓ Enabled Tooling Access");
            crate::output::emit(&ctx.out, &StatusView(resp))
        }
        ToolingAccessCmd::Disable => {
            let resp = ctx.sdk.admin.disable_tooling_access().await?;
            ctx.out.note("✓ Disabled Tooling Access");
            crate::output::emit(&ctx.out, &StatusView(resp))
        }
    }
}

#[derive(Serialize)]
struct StatusView(quicknode_sdk::ToolingAccessStatus);

impl Render for StatusView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![
            Cell::new("enabled"),
            Cell::new(self.0.enabled.to_string()),
        ]);
        t.add_row(vec![
            Cell::new("endpoint_url"),
            Cell::new(self.0.endpoint_url.as_deref().unwrap_or("-")),
        ]);
        t.add_row(vec![
            Cell::new("enabled_at"),
            Cell::new(self.0.enabled_at.as_deref().unwrap_or("-")),
        ]);
        write_table(w, &t)
    }
}
