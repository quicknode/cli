//! `qn team …` — manage teams and team members.

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    CreateTeamRequest, InviteTeamMemberRequest, RemoveTeamMemberRequest, UpdateTeamEndpointsRequest,
};
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};
use crate::retry::retrying;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: TeamCmd,
}

#[derive(Debug, Subcommand)]
pub enum TeamCmd {
    /// List all teams on the account.
    #[command(visible_alias = "ls")]
    List,
    /// Create a team.
    Create {
        /// Team name.
        #[arg(long)]
        name: String,
    },
    /// Show team detail.
    Show {
        /// Team id (numeric).
        #[arg(value_name = "TEAM_ID")]
        id: i64,
    },
    /// Delete a team.
    Delete {
        /// Team id (numeric).
        #[arg(value_name = "TEAM_ID")]
        id: i64,
    },
    /// List endpoints associated with a team.
    Endpoints {
        /// Team id (numeric).
        #[arg(value_name = "TEAM_ID")]
        id: i64,
    },
    /// Replace the set of endpoints associated with a team.
    SetEndpoints(SetEndpointsArgs),
    /// Manage team members.
    #[command(subcommand)]
    Member(MemberCmd),
}

#[derive(Debug, ClapArgs)]
pub struct SetEndpointsArgs {
    /// Team id (numeric).
    #[arg(value_name = "TEAM_ID")]
    pub id: i64,
    /// Endpoint ids to associate (pass each as an additional positional arg).
    pub endpoint_ids: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum MemberCmd {
    /// Invite a user to a team.
    Invite(InviteArgs),
    /// Remove a user from a team.
    Remove(RemoveArgs),
    /// Re-send a pending team invitation.
    Resend { team_id: i64, user_id: i64 },
}

#[derive(Debug, ClapArgs)]
pub struct RemoveArgs {
    pub team_id: i64,
    pub user_id: i64,
    /// Also delete the user account, not just remove them from the team.
    #[arg(long)]
    pub destroy_user: bool,
}

#[derive(Debug, ClapArgs)]
pub struct InviteArgs {
    /// Team id (numeric).
    pub team_id: i64,
    /// Email address.
    #[arg(long)]
    pub email: String,
    /// Full name (required for new users).
    #[arg(long)]
    pub full_name: Option<String>,
    /// Role to grant.
    #[arg(long, value_enum)]
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Role {
    Admin,
    Viewer,
    Billing,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Viewer => "viewer",
            Role::Billing => "billing",
        }
    }
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        TeamCmd::List => list(ctx).await,
        TeamCmd::Create { name } => create(name, ctx).await,
        TeamCmd::Show { id } => show(id, ctx).await,
        TeamCmd::Delete { id } => delete(id, ctx).await,
        TeamCmd::Endpoints { id } => endpoints(id, ctx).await,
        TeamCmd::SetEndpoints(a) => set_endpoints(a, ctx).await,
        TeamCmd::Member(c) => member(c, ctx).await,
    }
}

async fn list(ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.list_teams()).await?;
    crate::output::emit(&ctx.out, &TeamsView(resp))
}

async fn create(name: String, ctx: Ctx) -> Result<(), CliError> {
    let req = CreateTeamRequest { name: name.clone() };
    let resp = ctx.sdk.admin.create_team(&req).await?;
    if let Some(d) = &resp.data {
        ctx.out.note(&format!("✓ Created team {} ({})", d.id, name));
    }
    Ok(())
}

async fn show(id: i64, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_team(id)).await?;
    crate::output::emit(&ctx.out, &TeamDetailView(resp))
}

async fn delete(id: i64, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete team {id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.admin.delete_team(id).await?;
    ctx.out.note(&format!("✓ Deleted team {id}"));
    Ok(())
}

async fn endpoints(id: i64, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || ctx.sdk.admin.list_team_endpoints(id)).await?;
    crate::output::emit(&ctx.out, &TeamEndpointsView(resp))
}

async fn set_endpoints(a: SetEndpointsArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.endpoint_ids.is_empty() {
        return Err(CliError::Arg(
            "'team set-endpoints' requires at least one endpoint id.".into(),
        ));
    }
    let req = UpdateTeamEndpointsRequest {
        endpoint_ids: a.endpoint_ids.clone(),
    };
    ctx.sdk.admin.update_team_endpoints(a.id, &req).await?;
    ctx.out.note(&format!(
        "✓ Set {} endpoint(s) on team {}",
        a.endpoint_ids.len(),
        a.id
    ));
    Ok(())
}

async fn member(cmd: MemberCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        MemberCmd::Invite(a) => {
            let req = InviteTeamMemberRequest {
                email: a.email.clone(),
                full_name: a.full_name,
                role: a.role.map(|r| r.as_str().to_string()),
            };
            ctx.sdk.admin.invite_team_member(a.team_id, &req).await?;
            ctx.out
                .note(&format!("✓ Invited {} to team {}", a.email, a.team_id));
        }
        MemberCmd::Remove(a) => {
            let req = RemoveTeamMemberRequest {
                destroy_user: if a.destroy_user { Some(true) } else { None },
            };
            ctx.sdk
                .admin
                .remove_team_member(a.team_id, a.user_id, &req)
                .await?;
            ctx.out.note(&format!(
                "✓ Removed user {} from team {}",
                a.user_id, a.team_id
            ));
        }
        MemberCmd::Resend { team_id, user_id } => {
            ctx.sdk.admin.resend_team_invite(team_id, user_id).await?;
            ctx.out.note(&format!(
                "✓ Re-sent invite to user {user_id} on team {team_id}"
            ));
        }
    }
    Ok(())
}

// ----- renderers ----- //

#[derive(Serialize)]
struct TeamsView(quicknode_sdk::admin::ListTeamsResponse);

impl Render for TeamsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ID", "NAME", "MEMBERS"]);
        for tm in &self.0.data {
            t.add_row(vec![
                Cell::new(tm.id),
                Cell::new(&tm.name),
                opt_cell(&tm.members_count),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct TeamDetailView(quicknode_sdk::admin::GetTeamResponse);

impl Render for TeamDetailView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let detail = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(team not found)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(detail.id)]);
        t.add_row(vec![Cell::new("name"), Cell::new(&detail.name)]);
        t.add_row(vec![
            Cell::new("default_role"),
            opt_cell(&detail.default_role),
        ]);
        t.add_row(vec![
            Cell::new("members_count"),
            opt_cell(&detail.members_count),
        ]);
        write_table(w, &t)?;

        let member_section = |w: &mut dyn std::io::Write,
                              title: &str,
                              users: &[quicknode_sdk::admin::TeamUser]|
         -> std::io::Result<()> {
            if users.is_empty() {
                return Ok(());
            }
            writeln!(w)?;
            writeln!(w, "{} ({})", title, users.len())?;
            let mut t = new_table(ctx);
            set_header_bold(&mut t, ctx, vec!["ID", "EMAIL", "NAME", "ROLE", "STATUS"]);
            for u in users {
                t.add_row(vec![
                    Cell::new(u.id),
                    Cell::new(&u.email),
                    opt_cell(&u.full_name),
                    opt_cell(&u.role),
                    opt_cell(&u.status),
                ]);
            }
            write_table(w, &t)
        };
        member_section(w, "MEMBERS", &detail.users)?;
        member_section(w, "PENDING_INVITES", &detail.pending_invites)
    }
}

#[derive(Serialize)]
struct TeamEndpointsView(quicknode_sdk::admin::ListTeamEndpointsResponse);

impl Render for TeamEndpointsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ID", "SUBDOMAIN", "CHAIN", "NETWORK"]);
        for e in &self.0.data {
            t.add_row(vec![
                Cell::new(e.id),
                Cell::new(&e.subdomain),
                opt_cell(&e.chain),
                opt_cell(&e.network),
            ]);
        }
        write_table(w, &t)
    }
}
