//! `qn endpoint security …` — manage per-endpoint security settings.
//!
//! Client certificates, the method blacklist, range limits, and blocked IPs
//! live in [`controls`]; everything else is here.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    CreateDomainMaskRequest, CreateIpRequest, CreateJwtRequest,
    CreateOrUpdateIpCustomHeaderRequest, CreateReferrerRequest, CreateRequestFilterPatternRequest,
    CreateRequestFilterRequest, SecurityOptionsUpdate, UpdateRequestFilterPatternRequest,
    UpdateRequestFilterRequest, UpdateSecurityOptionsRequest,
};
use serde::Serialize;

mod controls;

use crate::confirm::confirm_mild;
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{
    bool_cell, new_table, opt_cell, set_header_bold, write_record, write_table, Render,
};
use crate::retry::retrying;

#[derive(Debug, Subcommand)]
pub enum SecurityCmd {
    /// Show the endpoint's full security configuration (tokens, referrers, IPs, ...).
    Show {
        /// Endpoint id.
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// List the security feature toggles and their current state.
    Options {
        /// Endpoint id.
        #[arg(value_name = "ENDPOINT_ID")]
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
    /// Manage client certificates (mTLS).
    #[command(subcommand)]
    ClientCert(controls::ClientCertCmd),
    /// Manage the blocked RPC method list.
    #[command(subcommand)]
    MethodBlacklist(controls::MethodBlacklistCmd),
    /// Manage per-method block-range limits.
    #[command(subcommand)]
    RangeLimit(controls::RangeLimitCmd),
    /// Manage blocked IP addresses and CIDR ranges.
    #[command(subcommand)]
    BlockedIp(controls::BlockedIpCmd),
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
    #[arg(value_name = "ENDPOINT_ID")]
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
    #[arg(long, value_enum)]
    pub response_logging: Option<Toggle>,
}

#[derive(Debug, Subcommand)]
pub enum TokenCmd {
    /// Generate a new auth token.
    Create {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Delete an auth token.
    Delete {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        token_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReferrerCmd {
    /// Whitelist a referrer URL or domain.
    Add {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        referrer: String,
    },
    /// Remove a referrer.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        referrer_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum IpCmd {
    /// Whitelist an IP address.
    Add {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        ip: String,
    },
    /// Remove an IP.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        ip_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum JwtCmd {
    /// Configure JWT validation. Supply the PEM public key inline or via --public-key-file.
    Add(JwtAddArgs),
    /// Remove a JWT configuration.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        jwt_id: String,
    },
}

#[derive(Debug, ClapArgs)]
pub struct JwtAddArgs {
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    /// PEM public key string.
    #[arg(long)]
    pub public_key: Option<String>,
    /// Path to a file containing the PEM public key.
    #[arg(long)]
    pub public_key_file: Option<PathBuf>,
    /// Key id (`kid`).
    #[arg(long)]
    pub kid: String,
    /// Human-readable name.
    #[arg(long)]
    pub name: String,
}

#[derive(Debug, Subcommand)]
pub enum DomainMaskCmd {
    /// Add a custom domain mask.
    Add {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        domain: String,
    },
    /// Remove a domain mask.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        domain_mask_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RequestFilterCmd {
    /// List request filters with their complete patterns.
    #[command(visible_alias = "ls")]
    List {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Create a filter: whitelist RPC methods (--method) or match a custom JSON pattern (--pattern).
    #[command(after_help = "Examples:\n  \
        qn endpoint security request-filter create ep-1 --method eth_call --method eth_blockNumber\n  \
        qn endpoint security request-filter create ep-1 \\\n    \
          --pattern '{\"method\":[{\"prefix\":\"eth_\"}],\"jsonrpc\":[\"2.0\"]}'\n  \
        qn endpoint security request-filter create ep-1 --pattern-file filter.json")]
    Create(RequestFilterCreateArgs),
    /// Replace an existing filter's whitelisted methods (--method) or custom pattern (--pattern).
    Update(RequestFilterUpdateArgs),
    /// Remove a request filter.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        request_filter_id: String,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RequestFilterCreateArgs {
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    /// RPC method to whitelist; pass --method multiple times or use --methods comma-list.
    #[arg(long = "method")]
    pub methods: Vec<String>,
    /// Comma-separated list of methods (alternative to repeated --method).
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
    /// Custom filter pattern as a JSON object. Mutually exclusive with --method.
    #[arg(long, value_name = "JSON")]
    pub pattern: Option<String>,
    /// Read the JSON pattern from a file, or from stdin when the path is `-`.
    #[arg(long, value_name = "PATH")]
    pub pattern_file: Option<PathBuf>,
}

#[derive(Debug, ClapArgs)]
pub struct RequestFilterUpdateArgs {
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    pub request_filter_id: String,
    #[arg(long = "method")]
    pub methods: Vec<String>,
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
    /// Custom filter pattern as a JSON object. Mutually exclusive with --method.
    #[arg(long, value_name = "JSON")]
    pub pattern: Option<String>,
    /// Read the JSON pattern from a file, or from stdin when the path is `-`.
    #[arg(long, value_name = "PATH")]
    pub pattern_file: Option<PathBuf>,
}

/// What a request-filter create/update sends: a method whitelist or a custom
/// pattern, never both.
enum FilterInput {
    Methods(Vec<String>),
    Pattern(HashMap<String, serde_json::Value>),
}

fn filter_input(
    methods: Vec<String>,
    methods_csv: Vec<String>,
    pattern: Option<String>,
    pattern_file: Option<PathBuf>,
) -> Result<FilterInput, CliError> {
    let mut methods = methods;
    methods.extend(methods_csv);
    let has_pattern = pattern.is_some() || pattern_file.is_some();
    if has_pattern && !methods.is_empty() {
        return Err(CliError::Arg(
            "supply either --method/--methods or --pattern/--pattern-file, not both".into(),
        ));
    }
    if !has_pattern {
        if methods.is_empty() {
            return Err(CliError::Arg(
                "supply at least one --method, or a --pattern/--pattern-file".into(),
            ));
        }
        return Ok(FilterInput::Methods(methods));
    }
    let text = read_inline_or_file(pattern, pattern_file, "pattern")?;
    let pattern: HashMap<String, serde_json::Value> = serde_json::from_str(&text)
        .map_err(|e| CliError::Arg(format!("--pattern must be a JSON object: {e}")))?;
    if pattern.is_empty() {
        return Err(CliError::Arg(
            "--pattern must not be an empty object".into(),
        ));
    }
    Ok(FilterInput::Pattern(pattern))
}

/// Resolves a value given inline or via a file path (`-` reads stdin). Errors
/// if both or neither are given.
pub(super) fn read_inline_or_file(
    inline: Option<String>,
    file: Option<PathBuf>,
    what: &str,
) -> Result<String, CliError> {
    match (inline, file) {
        (Some(s), None) => Ok(s),
        (None, Some(path)) if path.as_os_str() == "-" => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| CliError::Arg(format!("could not read {what} from stdin: {e}")))?;
            Ok(buf)
        }
        (None, Some(path)) => std::fs::read_to_string(&path).map_err(|e| {
            CliError::Arg(format!(
                "could not read {what} file '{}': {e}",
                path.display()
            ))
        }),
        (None, None) => Err(CliError::Arg(format!("supply --{what} or --{what}-file"))),
        (Some(_), Some(_)) => Err(CliError::Arg(format!(
            "supply only one of --{what} or --{what}-file"
        ))),
    }
}

#[derive(Debug, Subcommand)]
pub enum IpHeaderCmd {
    /// Set the custom header used to identify the client IP.
    Set {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        header_name: String,
    },
    /// Remove the custom IP header configuration.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
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
        SecurityCmd::ClientCert(c) => controls::client_cert(c, ctx).await,
        SecurityCmd::MethodBlacklist(c) => controls::method_blacklist(c, ctx).await,
        SecurityCmd::RangeLimit(c) => controls::range_limit(c, ctx).await,
        SecurityCmd::BlockedIp(c) => controls::blocked_ip(c, ctx).await,
    }
}

async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_endpoint_security(id)
    })
    .await?;
    crate::output::emit(&ctx.out, &SecurityShowView(resp))
}

async fn options_show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.admin.get_security_options(id)
    })
    .await?;
    crate::output::emit(&ctx.out, &SecurityOptionsView(resp))
}

async fn set_options(a: SetOptionsArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.tokens.is_none()
        && a.referrers.is_none()
        && a.jwts.is_none()
        && a.ips.is_none()
        && a.domain_masks.is_none()
        && a.hsts.is_none()
        && a.cors.is_none()
        && a.request_filters.is_none()
        && a.ip_custom_header.is_none()
        && a.response_logging.is_none()
    {
        return Err(CliError::Arg(
            "'endpoint security set-options' requires at least one of: \
             --tokens, --referrers, --jwts, --ips, --domain-masks, --hsts, \
             --cors, --request-filters, --ip-custom-header, --response-logging."
                .into(),
        ));
    }
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
        response_logging: a.response_logging.map(|t| t.as_str().to_string()),
    };
    let req = UpdateSecurityOptionsRequest { options };
    let resp = ctx.sdk.admin.update_security_options(&a.id, &req).await?;
    ctx.out
        .note(&format!("✓ Updated security options on {}", a.id));
    crate::output::emit(&ctx.out, &SecurityOptionsListView(resp.data))
}

/// Best-effort post-add check: if the security option governing the item just
/// added is disabled, the item is not enforced and won't appear in list output,
/// so warn on stderr with the exact enable command. Never fails the command —
/// any error fetching the options is swallowed (the add already succeeded),
/// and the lookup is skipped entirely under `--quiet`.
async fn warn_if_option_disabled(ctx: &Ctx, id: &str, option: &str, flag: &str, item_desc: &str) {
    if ctx.out.quiet {
        return;
    }
    let Ok(resp) = ctx.sdk.admin.get_security_options(id).await else {
        return;
    };
    if resp
        .data
        .iter()
        .any(|o| o.option == option && o.status == "disabled")
    {
        ctx.out.warn(&format!(
            "⚠ The '{option}' security option is disabled on {id} —\n  \
             {item_desc} will have no effect until you enable it:\n    \
             qn endpoint security set-options --{flag} enabled {id}"
        ));
    }
}

async fn token(cmd: TokenCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        TokenCmd::Create { id } => {
            let resp = ctx.sdk.admin.create_token_with_response(&id).await?;
            ctx.out
                .note(&format!("✓ Created token {} on {id}", resp.data.id));
            warn_if_option_disabled(&ctx, &id, "tokens", "tokens", "this token").await;
            crate::output::emit(&ctx.out, &TokenCreatedView(resp))?;
        }
        TokenCmd::Delete { id, token_id } => {
            confirm_mild(
                &ctx,
                &format!(
                    "Delete token {token_id} on {id}? Clients authenticating with it lose access"
                ),
            )?;
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
                referrer: referrer.clone(),
            };
            let resp = ctx
                .sdk
                .admin
                .create_referrer_with_response(&id, &req)
                .await?;
            ctx.out
                .note(&format!("✓ Whitelisted referrer {referrer:?} on {id}"));
            warn_if_option_disabled(&ctx, &id, "referrers", "referrers", "this referrer").await;
            crate::output::emit(&ctx.out, &ReferrerCreatedView(resp))?;
        }
        ReferrerCmd::Remove { id, referrer_id } => {
            confirm_mild(
                &ctx,
                &format!("Remove referrer {referrer_id} from endpoint {id}'s whitelist?"),
            )?;
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
            let req = CreateIpRequest { ip: ip.clone() };
            let resp = ctx.sdk.admin.create_ip_with_response(&id, &req).await?;
            ctx.out.note(&format!("✓ Whitelisted IP {ip} on {id}"));
            warn_if_option_disabled(&ctx, &id, "ips", "ips", "this IP").await;
            crate::output::emit(&ctx.out, &IpCreatedView(resp))?;
        }
        IpCmd::Remove { id, ip_id } => {
            confirm_mild(
                &ctx,
                &format!("Remove IP {ip_id} from endpoint {id}'s whitelist?"),
            )?;
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
                (Some(s), None) => s,
                (None, Some(p)) => std::fs::read_to_string(&p)?,
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
            let resp = ctx.sdk.admin.create_jwt_with_response(&a.id, &req).await?;
            ctx.out
                .note(&format!("✓ Added JWT {} on {}", resp.data.id, a.id));
            warn_if_option_disabled(&ctx, &a.id, "jwts", "jwts", "this JWT").await;
            crate::output::emit(&ctx.out, &JwtCreatedView(resp))?;
        }
        JwtCmd::Remove { id, jwt_id } => {
            confirm_mild(&ctx, &format!("Remove JWT {jwt_id} from endpoint {id}?"))?;
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
            let resp = ctx
                .sdk
                .admin
                .create_domain_mask_with_response(&id, &req)
                .await?;
            ctx.out
                .note(&format!("✓ Added domain mask {domain:?} on {id}"));
            warn_if_option_disabled(&ctx, &id, "domainMasks", "domain-masks", "this domain mask")
                .await;
            crate::output::emit(&ctx.out, &DomainMaskCreatedView(resp))?;
        }
        DomainMaskCmd::Remove { id, domain_mask_id } => {
            confirm_mild(
                &ctx,
                &format!("Remove domain mask {domain_mask_id} from endpoint {id}?"),
            )?;
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
        RequestFilterCmd::List { id } => {
            let resp = retrying(ctx.global.retries, || {
                ctx.sdk.admin.get_request_filters(&id)
            })
            .await?;
            crate::output::emit(&ctx.out, &RequestFiltersView(resp))?;
        }
        RequestFilterCmd::Create(a) => {
            let input = filter_input(a.methods, a.methods_csv, a.pattern, a.pattern_file)?;
            let resp = match input {
                FilterInput::Methods(method) => {
                    let req = CreateRequestFilterRequest { method };
                    ctx.sdk.admin.create_request_filter(&a.id, &req).await?
                }
                FilterInput::Pattern(pattern) => {
                    let req = CreateRequestFilterPatternRequest { pattern };
                    ctx.sdk
                        .admin
                        .create_request_filter_pattern(&a.id, &req)
                        .await?
                }
            };
            let d = resp.data.as_ref().ok_or_else(|| {
                CliError::Format("API returned success but no data; nothing was created".into())
            })?;
            ctx.out
                .note(&format!("✓ Created request filter {} on {}", d.id, a.id));
            warn_if_option_disabled(
                &ctx,
                &a.id,
                "requestFilters",
                "request-filters",
                "this request filter",
            )
            .await;
        }
        RequestFilterCmd::Update(a) => {
            let input = filter_input(a.methods, a.methods_csv, a.pattern, a.pattern_file)?;
            match input {
                FilterInput::Methods(method) => {
                    let req = UpdateRequestFilterRequest {
                        method: Some(method),
                    };
                    ctx.sdk
                        .admin
                        .update_request_filter(&a.id, &a.request_filter_id, &req)
                        .await?;
                }
                FilterInput::Pattern(pattern) => {
                    let req = UpdateRequestFilterPatternRequest { pattern };
                    ctx.sdk
                        .admin
                        .update_request_filter_pattern(&a.id, &a.request_filter_id, &req)
                        .await?;
                }
            }
            ctx.out.note(&format!(
                "✓ Updated request filter {} on {}",
                a.request_filter_id, a.id
            ));
        }
        RequestFilterCmd::Remove {
            id,
            request_filter_id,
        } => {
            confirm_mild(
                &ctx,
                &format!("Remove request filter {request_filter_id} from endpoint {id}?"),
            )?;
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
            warn_if_option_disabled(
                &ctx,
                &id,
                "ipCustomHeader",
                "ip-custom-header",
                "this header",
            )
            .await;
        }
        IpHeaderCmd::Remove { id } => {
            confirm_mild(
                &ctx,
                &format!("Remove the custom IP header configuration on endpoint {id}?"),
            )?;
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
        let opts = data.options.as_ref();
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["OPTION", "ENABLED"]);
        t.add_row(vec![
            Cell::new("tokens"),
            bool_cell(opts.and_then(|o| o.tokens)),
        ]);
        t.add_row(vec![
            Cell::new("jwts"),
            bool_cell(opts.and_then(|o| o.jwts)),
        ]);
        t.add_row(vec![
            Cell::new("domain_masks"),
            bool_cell(opts.and_then(|o| o.domain_masks)),
        ]);
        t.add_row(vec![Cell::new("ips"), bool_cell(opts.and_then(|o| o.ips))]);
        t.add_row(vec![
            Cell::new("referrers"),
            bool_cell(opts.and_then(|o| o.referrers)),
        ]);
        t.add_row(vec![
            Cell::new("request_filters"),
            bool_cell(opts.and_then(|o| o.request_filters)),
        ]);
        t.add_row(vec![
            Cell::new("hsts"),
            bool_cell(opts.and_then(|o| o.hsts)),
        ]);
        t.add_row(vec![
            Cell::new("cors"),
            bool_cell(opts.and_then(|o| o.cors)),
        ]);
        t.add_row(vec![
            Cell::new("response_logging"),
            bool_cell(opts.and_then(|o| o.response_logging)),
        ]);
        let ip_header = opts
            .and_then(|o| o.ip_custom_header.as_ref())
            .and_then(|h| h.value.clone());
        t.add_row(vec![Cell::new("ip_custom_header"), opt_cell(&ip_header)]);
        write_table(w, &t)?;
        security_item_sections(w, ctx, data)
    }
}

/// Renders one titled table per configured security item list (TOKENS, JWTS,
/// ...). Lists with no items render no section so lightly-configured
/// endpoints stay compact. Shared between `endpoint security show` and
/// `endpoint show`.
pub(crate) fn security_item_sections(
    w: &mut dyn std::io::Write,
    ctx: &crate::output::OutputCtx,
    data: &quicknode_sdk::admin::EndpointSecurity,
) -> std::io::Result<()> {
    let section = |w: &mut dyn std::io::Write,
                   title: &str,
                   headers: Vec<&str>,
                   rows: Vec<Vec<Cell>>|
     -> std::io::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        writeln!(w)?;
        writeln!(w, "{} ({})", title, rows.len())?;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, headers);
        for row in rows {
            t.add_row(row);
        }
        write_table(w, &t)
    };

    let tokens = data.tokens.as_deref().unwrap_or(&[]);
    section(
        w,
        "TOKENS",
        vec!["ID", "TOKEN"],
        tokens
            .iter()
            .map(|t| vec![Cell::new(&t.id), Cell::new(&t.token)])
            .collect(),
    )?;
    let jwts = data.jwts.as_deref().unwrap_or(&[]);
    section(
        w,
        "JWTS",
        vec!["ID", "NAME", "KID"],
        jwts.iter()
            .map(|j| vec![Cell::new(&j.id), Cell::new(&j.name), Cell::new(&j.kid)])
            .collect(),
    )?;
    let referrers = data.referrers.as_deref().unwrap_or(&[]);
    section(
        w,
        "REFERRERS",
        vec!["ID", "REFERRER"],
        referrers
            .iter()
            .map(|r| vec![Cell::new(&r.id), opt_cell(&r.referrer)])
            .collect(),
    )?;
    let masks = data.domain_masks.as_deref().unwrap_or(&[]);
    section(
        w,
        "DOMAIN_MASKS",
        vec!["ID", "DOMAIN"],
        masks
            .iter()
            .map(|d| vec![Cell::new(&d.id), Cell::new(&d.domain)])
            .collect(),
    )?;
    let ips = data.ips.as_deref().unwrap_or(&[]);
    section(
        w,
        "IPS",
        vec!["ID", "IP"],
        ips.iter()
            .map(|i| vec![Cell::new(&i.id), Cell::new(&i.ip)])
            .collect(),
    )?;
    let blocked = data.blocked_ips.as_deref().unwrap_or(&[]);
    section(
        w,
        "BLOCKED_IPS",
        vec!["ID", "IP"],
        blocked
            .iter()
            .map(|i| vec![Cell::new(&i.id), Cell::new(&i.ip)])
            .collect(),
    )?;
    let filters = data.request_filters.as_deref().unwrap_or(&[]);
    section(
        w,
        "REQUEST_FILTERS",
        vec!["ID", "METHODS", "PARAMS"],
        filters
            .iter()
            .map(|f| {
                vec![
                    Cell::new(&f.id),
                    Cell::new(matchers_text(&f.method)),
                    Cell::new(pattern_text(&f.params)),
                ]
            })
            .collect(),
    )
}

/// Renders request-filter matchers for a table cell: string matchers as-is,
/// structured matchers (e.g. `{"prefix":"eth_"}`) as compact JSON.
fn matchers_text(matchers: &[serde_json::Value]) -> String {
    if matchers.is_empty() {
        return "—".to_string();
    }
    matchers
        .iter()
        .map(|m| match m.as_str() {
            Some(s) => s.to_string(),
            None => m.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders a JSON pattern map as compact JSON with sorted keys so table
/// output is stable; `—` when empty.
fn pattern_text(pattern: &std::collections::HashMap<String, serde_json::Value>) -> String {
    if pattern.is_empty() {
        return "—".to_string();
    }
    let mut keys: Vec<&String> = pattern.keys().collect();
    keys.sort();
    let map: serde_json::Map<String, serde_json::Value> = keys
        .into_iter()
        .map(|k| (k.clone(), pattern[k].clone()))
        .collect();
    serde_json::Value::Object(map).to_string()
}

#[derive(Serialize)]
struct RequestFiltersView(quicknode_sdk::admin::GetRequestFiltersResponse);

impl Render for RequestFiltersView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ID", "WHITELIST", "PATTERN"]);
        for f in &self.0.data {
            t.add_row(vec![
                Cell::new(&f.id),
                bool_cell(Some(f.method_whitelist)),
                Cell::new(pattern_text(&f.pattern)),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct TokenCreatedView(quicknode_sdk::admin::CreateTokenResponse);

impl Render for TokenCreatedView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        write_record(
            w,
            ctx,
            vec!["ID", "TOKEN"],
            vec![Cell::new(&d.id), Cell::new(&d.token)],
        )
    }
}

#[derive(Serialize)]
struct ReferrerCreatedView(quicknode_sdk::admin::CreateReferrerResponse);

impl Render for ReferrerCreatedView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        write_record(
            w,
            ctx,
            vec!["ID", "REFERRER"],
            vec![Cell::new(&d.id), Cell::new(&d.url)],
        )
    }
}

#[derive(Serialize)]
struct IpCreatedView(quicknode_sdk::admin::CreateIpResponse);

impl Render for IpCreatedView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        write_record(
            w,
            ctx,
            vec!["ID", "IP"],
            vec![Cell::new(&d.id), Cell::new(&d.ip)],
        )
    }
}

#[derive(Serialize)]
struct DomainMaskCreatedView(quicknode_sdk::admin::CreateDomainMaskResponse);

impl Render for DomainMaskCreatedView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        write_record(
            w,
            ctx,
            vec!["ID", "DOMAIN"],
            vec![Cell::new(&d.id), Cell::new(&d.domain_mask)],
        )
    }
}

#[derive(Serialize)]
struct JwtCreatedView(quicknode_sdk::admin::CreateJwtResponse);

impl Render for JwtCreatedView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let d = &self.0.data;
        write_record(
            w,
            ctx,
            vec!["ID", "NAME", "KID"],
            vec![Cell::new(&d.id), Cell::new(&d.name), Cell::new(&d.kid)],
        )
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
