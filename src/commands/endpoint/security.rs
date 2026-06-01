//! `qn endpoint security …` — manage per-endpoint security settings.

use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    CreateDomainMaskRequest, CreateIpRequest, CreateJwtRequest,
    CreateOrUpdateIpCustomHeaderRequest, CreateReferrerRequest, CreateRequestFilterRequest,
    SecurityOptionsUpdate, UpdateRequestFilterRequest, UpdateSecurityOptionsRequest,
};
use serde::Serialize;

use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};

#[derive(Debug, Subcommand)]
pub enum SecurityCmd {
    /// Show the endpoint's full security configuration (tokens, referrers, IPs, ...).
    Show {
        /// Endpoint id.
        id: String,
    },
    /// List the security feature toggles and their current state.
    Options {
        /// Endpoint id.
        id: String,
    },
    /// Enable/disable individual security feature toggles.
    SetOptions(SetOptionsArgs),

    /// Manage authentication tokens on an endpoint.
    #[command(subcommand)]
    Token(TokenCmd),
    /// Manage referrer whitelist.
    #[command(subcommand)]
    Referrer(ReferrerCmd),
    /// Manage IP whitelist.
    #[command(subcommand)]
    Ip(IpCmd),
    /// Manage JWT validation keys.
    #[command(subcommand)]
    Jwt(JwtCmd),
    /// Manage custom domain masks.
    #[command(subcommand)]
    DomainMask(DomainMaskCmd),
    /// Manage RPC method request filters.
    #[command(subcommand)]
    RequestFilter(RequestFilterCmd),
    /// Manage the custom IP-identifying header.
    #[command(subcommand)]
    IpHeader(IpHeaderCmd),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Toggle {
    Enabled,
    Disabled,
}

impl Toggle {
    fn as_str(self) -> &'static str {
        match self {
            Toggle::Enabled => "enabled",
            Toggle::Disabled => "disabled",
        }
    }
}

#[derive(Debug, ClapArgs)]
pub struct SetOptionsArgs {
    pub id: String,
    #[arg(long, value_enum)]
    pub tokens: Option<Toggle>,
    #[arg(long, value_enum)]
    pub referrers: Option<Toggle>,
    #[arg(long, value_enum)]
    pub jwts: Option<Toggle>,
    #[arg(long, value_enum)]
    pub ips: Option<Toggle>,
    #[arg(long, value_enum)]
    pub domain_masks: Option<Toggle>,
    #[arg(long, value_enum)]
    pub hsts: Option<Toggle>,
    #[arg(long, value_enum)]
    pub cors: Option<Toggle>,
    #[arg(long, value_enum)]
    pub request_filters: Option<Toggle>,
    #[arg(long, value_enum)]
    pub ip_custom_header: Option<Toggle>,
}

#[derive(Debug, Subcommand)]
pub enum TokenCmd {
    /// Generate a new auth token.
    Create { id: String },
    /// Delete an auth token.
    Delete { id: String, token_id: String },
}

#[derive(Debug, Subcommand)]
pub enum ReferrerCmd {
    /// Whitelist a referrer URL or domain.
    Add { id: String, referrer: String },
    /// Remove a referrer.
    Remove { id: String, referrer_id: String },
}

#[derive(Debug, Subcommand)]
pub enum IpCmd {
    /// Whitelist an IP address.
    Add { id: String, ip: String },
    /// Remove an IP.
    Remove { id: String, ip_id: String },
}

#[derive(Debug, Subcommand)]
pub enum JwtCmd {
    /// Configure JWT validation. Supply the PEM public key inline or via --public-key-file.
    Add(JwtAddArgs),
    /// Remove a JWT configuration.
    Remove { id: String, jwt_id: String },
}

#[derive(Debug, ClapArgs)]
pub struct JwtAddArgs {
    pub id: String,
    /// PEM public key string.
    #[arg(long)]
    pub public_key: Option<String>,
    /// Path to a file containing the PEM public key.
    #[arg(long)]
    pub public_key_file: Option<PathBuf>,
    /// Key id (`kid`).
    #[arg(long)]
    pub kid: Option<String>,
    /// Human-readable name.
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum DomainMaskCmd {
    /// Add a custom domain mask.
    Add { id: String, domain: String },
    /// Remove a domain mask.
    Remove { id: String, domain_mask_id: String },
}

#[derive(Debug, Subcommand)]
pub enum RequestFilterCmd {
    /// Whitelist specific RPC methods. Pass --method repeatedly or a comma list.
    Create(RequestFilterCreateArgs),
    /// Replace the whitelisted RPC methods of an existing filter.
    Update(RequestFilterUpdateArgs),
    /// Remove a request filter.
    Remove {
        id: String,
        request_filter_id: String,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RequestFilterCreateArgs {
    pub id: String,
    /// RPC method to whitelist; pass --method multiple times or use --methods comma-list.
    #[arg(long = "method")]
    pub methods: Vec<String>,
    /// Comma-separated list of methods (alternative to repeated --method).
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
}

#[derive(Debug, ClapArgs)]
pub struct RequestFilterUpdateArgs {
    pub id: String,
    pub request_filter_id: String,
    #[arg(long = "method")]
    pub methods: Vec<String>,
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum IpHeaderCmd {
    /// Set the custom header used to identify the client IP.
    Set { id: String, header_name: String },
    /// Remove the custom IP header configuration.
    Remove { id: String },
}

pub async fn run(cmd: SecurityCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        SecurityCmd::Show { id } => show(&id, ctx).await,
        SecurityCmd::Options { id } => options_show(&id, ctx).await,
        SecurityCmd::SetOptions(a) => set_options(a, ctx).await,
        SecurityCmd::Token(c) => token(c, ctx).await,
        SecurityCmd::Referrer(c) => referrer(c, ctx).await,
        SecurityCmd::Ip(c) => ip(c, ctx).await,
        SecurityCmd::Jwt(c) => jwt(c, ctx).await,
        SecurityCmd::DomainMask(c) => domain_mask(c, ctx).await,
        SecurityCmd::RequestFilter(c) => request_filter(c, ctx).await,
        SecurityCmd::IpHeader(c) => ip_header(c, ctx).await,
    }
}

async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.get_endpoint_security(id).await?;
    crate::output::emit(&ctx.out, &SecurityShowView(resp))
}

async fn options_show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.admin.get_security_options(id).await?;
    crate::output::emit(&ctx.out, &SecurityOptionsView(resp))
}

async fn set_options(a: SetOptionsArgs, ctx: Ctx) -> Result<(), CliError> {
    let options = SecurityOptionsUpdate {
        tokens: a.tokens.map(|t| t.as_str().to_string()),
        referrers: a.referrers.map(|t| t.as_str().to_string()),
        jwts: a.jwts.map(|t| t.as_str().to_string()),
        ips: a.ips.map(|t| t.as_str().to_string()),
        domain_masks: a.domain_masks.map(|t| t.as_str().to_string()),
        hsts: a.hsts.map(|t| t.as_str().to_string()),
        cors: a.cors.map(|t| t.as_str().to_string()),
        request_filters: a.request_filters.map(|t| t.as_str().to_string()),
        ip_custom_header: a.ip_custom_header.map(|t| t.as_str().to_string()),
    };
    let req = UpdateSecurityOptionsRequest { options };
    let resp = ctx.sdk.admin.update_security_options(&a.id, &req).await?;
    ctx.out
        .note(&format!("✓ Updated security options on {}", a.id));
    crate::output::emit(&ctx.out, &SecurityOptionsListView(resp.data))
}

async fn token(cmd: TokenCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        TokenCmd::Create { id } => {
            ctx.sdk.admin.create_token(&id).await?;
            ctx.out.note(&format!("✓ Created token on {id}"));
        }
        TokenCmd::Delete { id, token_id } => {
            ctx.sdk.admin.delete_token(&id, &token_id).await?;
            ctx.out.note(&format!("✓ Deleted token {token_id} on {id}"));
        }
    }
    Ok(())
}

async fn referrer(cmd: ReferrerCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        ReferrerCmd::Add { id, referrer } => {
            let req = CreateReferrerRequest {
                referrer: Some(referrer.clone()),
            };
            ctx.sdk.admin.create_referrer(&id, &req).await?;
            ctx.out
                .note(&format!("✓ Whitelisted referrer {referrer:?} on {id}"));
        }
        ReferrerCmd::Remove { id, referrer_id } => {
            ctx.sdk.admin.delete_referrer(&id, &referrer_id).await?;
            ctx.out
                .note(&format!("✓ Removed referrer {referrer_id} on {id}"));
        }
    }
    Ok(())
}

async fn ip(cmd: IpCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        IpCmd::Add { id, ip } => {
            let req = CreateIpRequest {
                ip: Some(ip.clone()),
            };
            ctx.sdk.admin.create_ip(&id, &req).await?;
            ctx.out.note(&format!("✓ Whitelisted IP {ip} on {id}"));
        }
        IpCmd::Remove { id, ip_id } => {
            ctx.sdk.admin.delete_ip(&id, &ip_id).await?;
            ctx.out.note(&format!("✓ Removed IP {ip_id} on {id}"));
        }
    }
    Ok(())
}

async fn jwt(cmd: JwtCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        JwtCmd::Add(a) => {
            let public_key = match (a.public_key, a.public_key_file) {
                (Some(s), None) => Some(s),
                (None, Some(p)) => Some(std::fs::read_to_string(&p)?),
                (None, None) => {
                    return Err(CliError::Arg(
                        "supply --public-key or --public-key-file".to_string(),
                    ));
                }
                (Some(_), Some(_)) => {
                    return Err(CliError::Arg(
                        "supply only one of --public-key or --public-key-file".to_string(),
                    ));
                }
            };
            let req = CreateJwtRequest {
                public_key,
                kid: a.kid,
                name: a.name,
            };
            ctx.sdk.admin.create_jwt(&a.id, &req).await?;
            ctx.out.note(&format!("✓ Added JWT on {}", a.id));
        }
        JwtCmd::Remove { id, jwt_id } => {
            ctx.sdk.admin.delete_jwt(&id, &jwt_id).await?;
            ctx.out.note(&format!("✓ Removed JWT {jwt_id} on {id}"));
        }
    }
    Ok(())
}

async fn domain_mask(cmd: DomainMaskCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        DomainMaskCmd::Add { id, domain } => {
            let req = CreateDomainMaskRequest {
                domain_mask: Some(domain.clone()),
            };
            ctx.sdk.admin.create_domain_mask(&id, &req).await?;
            ctx.out
                .note(&format!("✓ Added domain mask {domain:?} on {id}"));
        }
        DomainMaskCmd::Remove { id, domain_mask_id } => {
            ctx.sdk
                .admin
                .delete_domain_mask(&id, &domain_mask_id)
                .await?;
            ctx.out
                .note(&format!("✓ Removed domain mask {domain_mask_id} on {id}"));
        }
    }
    Ok(())
}

async fn request_filter(cmd: RequestFilterCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        RequestFilterCmd::Create(a) => {
            let mut methods = a.methods;
            methods.extend(a.methods_csv);
            if methods.is_empty() {
                return Err(CliError::Arg("supply at least one --method".to_string()));
            }
            let req = CreateRequestFilterRequest {
                method: Some(methods),
            };
            let resp = ctx.sdk.admin.create_request_filter(&a.id, &req).await?;
            let d = resp.data.as_ref().ok_or_else(|| {
                CliError::Format("API returned success but no data; nothing was created".into())
            })?;
            ctx.out
                .note(&format!("✓ Created request filter {} on {}", d.id, a.id));
        }
        RequestFilterCmd::Update(a) => {
            let mut methods = a.methods;
            methods.extend(a.methods_csv);
            let req = UpdateRequestFilterRequest {
                method: Some(methods),
            };
            ctx.sdk
                .admin
                .update_request_filter(&a.id, &a.request_filter_id, &req)
                .await?;
            ctx.out.note(&format!(
                "✓ Updated request filter {} on {}",
                a.request_filter_id, a.id
            ));
        }
        RequestFilterCmd::Remove {
            id,
            request_filter_id,
        } => {
            ctx.sdk
                .admin
                .delete_request_filter(&id, &request_filter_id)
                .await?;
            ctx.out.note(&format!(
                "✓ Removed request filter {request_filter_id} on {id}"
            ));
        }
    }
    Ok(())
}

async fn ip_header(cmd: IpHeaderCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        IpHeaderCmd::Set { id, header_name } => {
            let req = CreateOrUpdateIpCustomHeaderRequest {
                header_name: header_name.clone(),
            };
            ctx.sdk
                .admin
                .create_or_update_ip_custom_header(&id, &req)
                .await?;
            ctx.out
                .note(&format!("✓ Set IP header {header_name:?} on {id}"));
        }
        IpHeaderCmd::Remove { id } => {
            ctx.sdk.admin.delete_ip_custom_header(&id).await?;
            ctx.out.note(&format!("✓ Removed IP header config on {id}"));
        }
    }
    Ok(())
}

// ----- renderers ----- //

#[derive(Serialize)]
struct SecurityShowView(quicknode_sdk::admin::GetEndpointSecurityResponse);

impl Render for SecurityShowView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = match &self.0.data {
            Some(d) => d,
            None => {
                writeln!(w, "(no security data)")?;
                return Ok(());
            }
        };
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FEATURE", "COUNT", "DETAIL"]);
        let tokens = data.tokens.as_deref().unwrap_or(&[]);
        t.add_row(vec![
            Cell::new("tokens"),
            Cell::new(tokens.len()),
            Cell::new(""),
        ]);
        let referrers = data.referrers.as_deref().unwrap_or(&[]);
        t.add_row(vec![
            Cell::new("referrers"),
            Cell::new(referrers.len()),
            Cell::new(
                referrers
                    .iter()
                    .filter_map(|r| r.referrer.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]);
        let ips = data.ips.as_deref().unwrap_or(&[]);
        t.add_row(vec![
            Cell::new("ips"),
            Cell::new(ips.len()),
            Cell::new(
                ips.iter()
                    .map(|i| i.ip.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]);
        let jwts = data.jwts.as_deref().unwrap_or(&[]);
        t.add_row(vec![
            Cell::new("jwts"),
            Cell::new(jwts.len()),
            Cell::new(""),
        ]);
        let masks = data.domain_masks.as_deref().unwrap_or(&[]);
        t.add_row(vec![
            Cell::new("domain_masks"),
            Cell::new(masks.len()),
            Cell::new(
                masks
                    .iter()
                    .map(|d| d.domain.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]);
        let filters = data.request_filters.as_deref().unwrap_or(&[]);
        t.add_row(vec![
            Cell::new("request_filters"),
            Cell::new(filters.len()),
            Cell::new(""),
        ]);
        let ip_header = data
            .options
            .as_ref()
            .and_then(|o| o.ip_custom_header.as_ref())
            .and_then(|h| h.value.clone());
        t.add_row(vec![
            Cell::new("ip_custom_header"),
            opt_cell(&ip_header),
            Cell::new(""),
        ]);
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct SecurityOptionsView(quicknode_sdk::admin::GetSecurityOptionsResponse);

impl Render for SecurityOptionsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["OPTION", "STATUS", "VALUE"]);
        for o in &self.0.data {
            t.add_row(vec![
                Cell::new(&o.option),
                Cell::new(&o.status),
                opt_cell(&o.value),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct SecurityOptionsListView(Vec<quicknode_sdk::admin::SecurityOption>);

impl Render for SecurityOptionsListView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["OPTION", "STATUS", "VALUE"]);
        for o in &self.0 {
            t.add_row(vec![
                Cell::new(&o.option),
                Cell::new(&o.status),
                opt_cell(&o.value),
            ]);
        }
        write_table(w, &t)
    }
}
