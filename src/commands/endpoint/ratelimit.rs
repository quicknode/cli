//! `qn endpoint ratelimit …` and `qn endpoint method-ratelimit …`.

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    CreateMethodRateLimitRequest, RateLimitSettings, UpdateMethodRateLimitRequest,
    UpdateRateLimitsRequest,
};
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};

#[derive(Debug, Subcommand)]
pub enum RateLimitCmd {
    /// Show the endpoint's current per-bucket rate limits.
    Get { id: String },
    /// Set endpoint-level rate limits (any of rps/rpm/rpd; omitted are untouched).
    Set(SetArgs),
    /// Delete a user-set rate-limit override (plan defaults can't be deleted).
    DeleteOverride { id: String, override_id: String },

    /// List method-level rate limiters on the endpoint.
    MethodList { id: String },
    /// Create a new method-level rate limiter.
    MethodCreate(MethodCreateArgs),
    /// Update an existing method-level rate limiter.
    MethodUpdate(MethodUpdateArgs),
    /// Delete a method-level rate limiter.
    MethodDelete {
        id: String,
        method_rate_limit_id: String,
    },
}

#[derive(Debug, ClapArgs)]
pub struct SetArgs {
    pub id: String,
    /// Requests-per-second cap.
    #[arg(long)]
    pub rps: Option<i32>,
    /// Requests-per-minute cap.
    #[arg(long)]
    pub rpm: Option<i32>,
    /// Requests-per-day cap.
    #[arg(long)]
    pub rpd: Option<i32>,
}

#[derive(Debug, ClapArgs)]
pub struct MethodCreateArgs {
    pub id: String,
    /// Interval (second/minute/hour/day).
    #[arg(long, value_parser = ["second", "minute", "hour", "day"])]
    pub interval: String,
    /// RPC method (pass --method repeatedly).
    #[arg(long = "method")]
    pub methods: Vec<String>,
    /// Methods as a comma-separated list.
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
    /// Rate (max calls per interval).
    #[arg(long)]
    pub rate: i32,
}

#[derive(Debug, ClapArgs)]
pub struct MethodUpdateArgs {
    pub id: String,
    pub method_rate_limit_id: String,
    #[arg(long = "method")]
    pub methods: Vec<String>,
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
    #[arg(long, value_enum)]
    pub status: Option<StatusToggle>,
    #[arg(long)]
    pub rate: Option<i32>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StatusToggle {
    Enabled,
    Disabled,
}

impl StatusToggle {
    fn as_str(self) -> &'static str {
        match self {
            StatusToggle::Enabled => "enabled",
            StatusToggle::Disabled => "disabled",
        }
    }
}

pub async fn run(cmd: RateLimitCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        RateLimitCmd::Get { id } => get(&id, ctx).await,
        RateLimitCmd::Set(a) => set(a, ctx).await,
        RateLimitCmd::DeleteOverride { id, override_id } => {
            ctx.sdk
                .admin
                .delete_rate_limit_override(&id, &override_id)
                .await?;
            ctx.out.note(&format!(
                "✓ Deleted rate-limit override {override_id} on {id}"
            ));
            Ok(())
        }
        RateLimitCmd::MethodList { id } => method_list(&id, ctx).await,
        RateLimitCmd::MethodCreate(a) => method_create(a, ctx).await,
        RateLimitCmd::MethodUpdate(a) => method_update(a, ctx).await,
        RateLimitCmd::MethodDelete {
            id,
            method_rate_limit_id,
        } => {
            ctx.sdk
                .admin
                .delete_method_rate_limit(&id, &method_rate_limit_id)
                .await?;
            ctx.out.note(&format!(
                "✓ Deleted method rate limiter {method_rate_limit_id} on {id}"
            ));
            Ok(())
        }
    }
}

async fn get(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.get_rate_limits(id).await?;
    crate::output::emit(&ctx.out, &RateLimitsView(resp))
}

async fn set(a: SetArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.rps.is_none() && a.rpm.is_none() && a.rpd.is_none() {
        return Err(CliError::Arg(
            "supply at least one of --rps, --rpm, --rpd".to_string(),
        ));
    }
    let req = UpdateRateLimitsRequest {
        rate_limits: RateLimitSettings {
            rps: a.rps,
            rpm: a.rpm,
            rpd: a.rpd,
        },
    };
    ctx.sdk.admin.update_rate_limits(&a.id, &req).await?;
    ctx.out.note(&format!("✓ Updated rate limits on {}", a.id));
    Ok(())
}

async fn method_list(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.get_method_rate_limits(id).await?;
    crate::output::emit(&ctx.out, &MethodRateLimitsView(resp))
}

async fn method_create(a: MethodCreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let mut methods = a.methods;
    methods.extend(a.methods_csv);
    if methods.is_empty() {
        return Err(CliError::Arg("supply at least one --method".to_string()));
    }
    let req = CreateMethodRateLimitRequest {
        interval: a.interval,
        methods,
        rate: a.rate,
    };
    let resp = ctx.sdk.admin.create_method_rate_limit(&a.id, &req).await?;
    if let Some(d) = &resp.data {
        ctx.out.note(&format!(
            "✓ Created method rate limiter {} on {}",
            d.id, a.id
        ));
    }
    Ok(())
}

async fn method_update(a: MethodUpdateArgs, ctx: Ctx) -> Result<(), CliError> {
    let mut methods = a.methods;
    methods.extend(a.methods_csv);
    let req = UpdateMethodRateLimitRequest {
        methods: if methods.is_empty() {
            None
        } else {
            Some(methods)
        },
        status: a.status.map(|s| s.as_str().to_string()),
        rate: a.rate,
    };
    ctx.sdk
        .admin
        .update_method_rate_limit(&a.id, &a.method_rate_limit_id, &req)
        .await?;
    ctx.out.note(&format!(
        "✓ Updated method rate limiter {} on {}",
        a.method_rate_limit_id, a.id
    ));
    Ok(())
}

// ----- renderers ----- //

#[derive(Serialize)]
struct RateLimitsView(quicknode_sdk::admin::GetRateLimitsResponse);

impl Render for RateLimitsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no rate-limit data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["BUCKET", "VALUE", "SOURCE", "OVERRIDE_ID"],
        );
        for r in &data.rate_limits {
            t.add_row(vec![
                Cell::new(&r.bucket),
                Cell::new(r.rate_limit),
                Cell::new(&r.source),
                opt_cell(&r.id),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct MethodRateLimitsView(quicknode_sdk::admin::GetMethodRateLimitsResponse);

impl Render for MethodRateLimitsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no method rate limiters)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "INTERVAL", "METHODS", "RATE", "STATUS"],
        );
        for r in &data.rate_limiters {
            t.add_row(vec![
                Cell::new(&r.id),
                Cell::new(&r.interval),
                Cell::new(r.methods.join(", ")),
                Cell::new(r.rate),
                Cell::new(&r.status),
            ]);
        }
        write_table(w, &t)
    }
}
