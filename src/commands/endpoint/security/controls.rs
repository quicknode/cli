//! `qn endpoint security client-cert|method-blacklist|range-limit|blocked-ip …`
//! — the per-endpoint traffic controls.
//!
//! `set` verbs replace the whole configuration; `clear` is a separate verb so
//! a forgotten flag can never wipe it.

use std::path::PathBuf;

use clap::{ArgGroup, Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::admin::{
    ClientCertsData, CreateBlockedIpRequest, CreateClientCertRequest, CreateClientCertResponse,
    DeleteClientCertResponse, GetClientCertsResponse, GetMethodBlacklistResponse,
    GetRangeLimitsResponse, MethodBlacklistData, RangeLimitsData, UpdateClientCertsRequest,
    UpdateClientCertsResponse, UpdateMethodBlacklistRequest, UpdateMethodBlacklistResponse,
    UpdateRangeLimitsRequest, UpdateRangeLimitsResponse,
};
use serde::Serialize;

use crate::confirm::confirm_mild;
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, write_record, write_table, Render};
use crate::retry::retrying;

use super::read_inline_or_file;

#[derive(Debug, Subcommand)]
#[command(after_help = "Examples:\n  \
    qn endpoint security client-cert list ep-1\n  \
    qn endpoint security client-cert add ep-1 --certificate-file client.pem\n  \
    qn endpoint security client-cert enable ep-1\n  \
    qn endpoint security client-cert remove ep-1 ab:cd:ef --yes")]
pub enum ClientCertCmd {
    /// List client certificates and whether certificate authentication is enabled.
    #[command(visible_alias = "ls")]
    List {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Add a PEM client certificate. Supply it inline or via --certificate-file.
    Add(ClientCertAddArgs),
    /// Require a client certificate on every request.
    Enable {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Stop requiring client certificates (the certificates are kept).
    Disable {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Remove a client certificate by fingerprint.
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        fingerprint: String,
    },
}

#[derive(Debug, ClapArgs)]
#[command(group(ArgGroup::new("cert_source").args(["certificate", "certificate_file"]).required(true)))]
pub struct ClientCertAddArgs {
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    /// PEM-encoded certificate.
    // PEM text starts with `-----BEGIN`, which clap would otherwise read as a flag.
    #[arg(long, value_name = "PEM", allow_hyphen_values = true)]
    pub certificate: Option<String>,
    /// Path to a PEM file, or `-` for stdin.
    #[arg(long, value_name = "PATH")]
    pub certificate_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
#[command(after_help = "Examples:\n  \
    qn endpoint security method-blacklist show ep-1\n  \
    qn endpoint security method-blacklist set ep-1 --method eth_sendRawTransaction\n  \
    qn endpoint security method-blacklist clear ep-1 --yes")]
pub enum MethodBlacklistCmd {
    /// Show the blocked RPC methods.
    Show {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Replace the blocked RPC methods. Pass --method repeatedly or --methods a,b.
    Set(MethodBlacklistSetArgs),
    /// Unblock every method.
    Clear {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
}

#[derive(Debug, ClapArgs)]
pub struct MethodBlacklistSetArgs {
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    /// RPC method to block (repeatable).
    #[arg(long = "method")]
    pub methods: Vec<String>,
    /// Comma-separated list of methods to block.
    #[arg(long = "methods", value_delimiter = ',')]
    pub methods_csv: Vec<String>,
}

#[derive(Debug, Subcommand)]
#[command(after_help = "Examples:\n  \
    qn endpoint security range-limit show ep-1\n  \
    qn endpoint security range-limit set ep-1 --limit eth_getLogs=1000 --limit eth_getFilterLogs=500\n  \
    qn endpoint security range-limit clear ep-1 --yes")]
pub enum RangeLimitCmd {
    /// Show the per-method block-range limits and the methods that support one.
    Show {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Replace the block-range limits. Pass --limit METHOD=BLOCKS repeatedly.
    Set(RangeLimitSetArgs),
    /// Remove every block-range limit.
    Clear {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RangeLimitSetArgs {
    #[arg(value_name = "ENDPOINT_ID")]
    pub id: String,
    /// Maximum block range for a method, as `METHOD=BLOCKS` (repeatable).
    #[arg(long = "limit", value_name = "METHOD=BLOCKS", required = true)]
    pub limits: Vec<String>,
}

#[derive(Debug, Subcommand)]
#[command(after_help = "Examples:\n  \
    qn endpoint security blocked-ip list ep-1\n  \
    qn endpoint security blocked-ip add ep-1 192.0.2.0/24\n  \
    qn endpoint security blocked-ip remove ep-1 bip-1 --yes")]
pub enum BlockedIpCmd {
    /// List blocked IP addresses and CIDR ranges.
    #[command(visible_alias = "ls")]
    List {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
    },
    /// Block an IP address or CIDR range. Enforced while the `ips` security option is enabled.
    Add {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        ip: String,
    },
    /// Unblock an IP by its record id (from `blocked-ip list`).
    Remove {
        #[arg(value_name = "ENDPOINT_ID")]
        id: String,
        ip_id: String,
    },
}

pub async fn client_cert(cmd: ClientCertCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        ClientCertCmd::List { id } => {
            let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_client_certs(&id)).await?;
            crate::output::emit(&ctx.out, &ClientCertsView(resp))
        }
        ClientCertCmd::Add(a) => {
            let certificate =
                read_inline_or_file(a.certificate, a.certificate_file, "certificate")?;
            let req = CreateClientCertRequest { certificate };
            let resp = ctx.sdk.admin.create_client_cert(&a.id, &req).await?;
            ctx.out
                .note(&format!("✓ Added client certificate on {}", a.id));
            crate::output::emit(&ctx.out, &ClientCertsView(resp))
        }
        ClientCertCmd::Enable { id } => {
            let req = UpdateClientCertsRequest { enabled: true };
            let resp = ctx.sdk.admin.update_client_certs(&id, &req).await?;
            ctx.out.note(&format!(
                "✓ Enabled client-certificate authentication on {id}"
            ));
            crate::output::emit(&ctx.out, &ClientCertsView(resp))
        }
        ClientCertCmd::Disable { id } => {
            confirm_mild(
                &ctx,
                &format!(
                    "Disable client-certificate authentication on endpoint {id}? \
                     Requests without a certificate will be accepted"
                ),
            )?;
            let req = UpdateClientCertsRequest { enabled: false };
            let resp = ctx.sdk.admin.update_client_certs(&id, &req).await?;
            ctx.out.note(&format!(
                "✓ Disabled client-certificate authentication on {id}"
            ));
            crate::output::emit(&ctx.out, &ClientCertsView(resp))
        }
        ClientCertCmd::Remove { id, fingerprint } => {
            confirm_mild(
                &ctx,
                &format!(
                    "Remove client certificate {fingerprint} from endpoint {id}? \
                     Clients presenting it lose access"
                ),
            )?;
            let resp = ctx.sdk.admin.delete_client_cert(&id, &fingerprint).await?;
            ctx.out.note(&format!(
                "✓ Removed client certificate {fingerprint} on {id}"
            ));
            crate::output::emit(&ctx.out, &ClientCertsView(resp))
        }
    }
}

pub async fn method_blacklist(cmd: MethodBlacklistCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        MethodBlacklistCmd::Show { id } => {
            let resp = retrying(ctx.global.retries, || {
                ctx.sdk.admin.get_method_blacklist(&id)
            })
            .await?;
            crate::output::emit(&ctx.out, &MethodBlacklistView(resp))
        }
        MethodBlacklistCmd::Set(a) => {
            let mut methods = a.methods;
            methods.extend(a.methods_csv);
            if methods.is_empty() {
                return Err(CliError::Arg(
                    "supply at least one --method (use 'method-blacklist clear' to unblock all)"
                        .into(),
                ));
            }
            let req = UpdateMethodBlacklistRequest { methods };
            let resp = ctx.sdk.admin.update_method_blacklist(&a.id, &req).await?;
            ctx.out
                .note(&format!("✓ Replaced the method blacklist on {}", a.id));
            crate::output::emit(&ctx.out, &MethodBlacklistView(resp))
        }
        MethodBlacklistCmd::Clear { id } => {
            confirm_mild(
                &ctx,
                &format!(
                    "Clear the method blacklist on endpoint {id}? \
                     Every blocked method becomes callable"
                ),
            )?;
            let req = UpdateMethodBlacklistRequest { methods: vec![] };
            let resp = ctx.sdk.admin.update_method_blacklist(&id, &req).await?;
            ctx.out
                .note(&format!("✓ Cleared the method blacklist on {id}"));
            crate::output::emit(&ctx.out, &MethodBlacklistView(resp))
        }
    }
}

pub async fn range_limit(cmd: RangeLimitCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        RangeLimitCmd::Show { id } => {
            let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_range_limits(&id)).await?;
            crate::output::emit(&ctx.out, &RangeLimitsView(resp))
        }
        RangeLimitCmd::Set(a) => {
            let limits = a
                .limits
                .iter()
                .map(|pair| parse_range_limit(pair))
                .collect::<Result<_, _>>()?;
            let req = UpdateRangeLimitsRequest { limits };
            let resp = ctx.sdk.admin.update_range_limits(&a.id, &req).await?;
            ctx.out
                .note(&format!("✓ Replaced the block-range limits on {}", a.id));
            crate::output::emit(&ctx.out, &RangeLimitsView(resp))
        }
        RangeLimitCmd::Clear { id } => {
            confirm_mild(
                &ctx,
                &format!(
                    "Clear every block-range limit on endpoint {id}? \
                     Range-limited methods accept unbounded ranges"
                ),
            )?;
            let req = UpdateRangeLimitsRequest {
                limits: Default::default(),
            };
            let resp = ctx.sdk.admin.update_range_limits(&id, &req).await?;
            ctx.out
                .note(&format!("✓ Cleared the block-range limits on {id}"));
            crate::output::emit(&ctx.out, &RangeLimitsView(resp))
        }
    }
}

/// Parses one `METHOD=BLOCKS` pair; the API requires a positive block count.
fn parse_range_limit(pair: &str) -> Result<(String, i64), CliError> {
    let (method, blocks) = pair.split_once('=').ok_or_else(|| {
        CliError::Arg(format!(
            "--limit {pair:?} is not METHOD=BLOCKS (e.g. eth_getLogs=1000)"
        ))
    })?;
    let method = method.trim();
    let blocks: i64 = blocks.trim().parse().map_err(|_| {
        CliError::Arg(format!(
            "--limit {pair:?}: {blocks:?} is not a whole number of blocks"
        ))
    })?;
    if method.is_empty() || blocks <= 0 {
        return Err(CliError::Arg(format!(
            "--limit {pair:?}: the method must be non-empty and the block count positive"
        )));
    }
    Ok((method.to_string(), blocks))
}

pub async fn blocked_ip(cmd: BlockedIpCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        BlockedIpCmd::List { id } => {
            let resp = retrying(ctx.global.retries, || ctx.sdk.admin.get_blocked_ips(&id)).await?;
            crate::output::emit(&ctx.out, &BlockedIpsView(resp))
        }
        BlockedIpCmd::Add { id, ip } => {
            let req = CreateBlockedIpRequest { ip: ip.clone() };
            let resp = ctx.sdk.admin.create_blocked_ip(&id, &req).await?;
            ctx.out.note(&format!("✓ Blocked {ip} on {id}"));
            crate::output::emit(&ctx.out, &BlockedIpCreatedView(resp))
        }
        BlockedIpCmd::Remove { id, ip_id } => {
            confirm_mild(
                &ctx,
                &format!(
                    "Unblock IP {ip_id} on endpoint {id}? Requests from it will be accepted again"
                ),
            )?;
            ctx.sdk.admin.delete_blocked_ip(&id, &ip_id).await?;
            ctx.out.note(&format!("✓ Unblocked {ip_id} on {id}"));
            Ok(())
        }
    }
}

// ----- renderers ----- //

/// Lets one view render the several SDK response types that carry the same
/// payload (the four client-cert responses, the two blacklist responses, …).
trait Carries<D> {
    fn data(&self) -> &D;
}

impl Carries<ClientCertsData> for GetClientCertsResponse {
    fn data(&self) -> &ClientCertsData {
        &self.data
    }
}

impl Carries<ClientCertsData> for CreateClientCertResponse {
    fn data(&self) -> &ClientCertsData {
        &self.data
    }
}

impl Carries<ClientCertsData> for UpdateClientCertsResponse {
    fn data(&self) -> &ClientCertsData {
        &self.data
    }
}

impl Carries<ClientCertsData> for DeleteClientCertResponse {
    fn data(&self) -> &ClientCertsData {
        &self.data
    }
}

impl Carries<MethodBlacklistData> for GetMethodBlacklistResponse {
    fn data(&self) -> &MethodBlacklistData {
        &self.data
    }
}

impl Carries<MethodBlacklistData> for UpdateMethodBlacklistResponse {
    fn data(&self) -> &MethodBlacklistData {
        &self.data
    }
}

impl Carries<RangeLimitsData> for GetRangeLimitsResponse {
    fn data(&self) -> &RangeLimitsData {
        &self.data
    }
}

impl Carries<RangeLimitsData> for UpdateRangeLimitsResponse {
    fn data(&self) -> &RangeLimitsData {
        &self.data
    }
}

#[derive(Serialize)]
struct ClientCertsView<T>(T);

impl<T: Serialize + Carries<ClientCertsData>> Render for ClientCertsView<T> {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = self.0.data();
        writeln!(w, "enabled: {}", if data.enabled { "✓" } else { "✗" })?;
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["FINGERPRINT", "SUBJECT", "ISSUER", "EXPIRES_AT"],
        );
        for c in &data.certs {
            t.add_row(vec![
                Cell::new(&c.fingerprint),
                Cell::new(&c.subject),
                Cell::new(&c.issuer),
                Cell::new(&c.expires_at),
            ]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct MethodBlacklistView<T>(T);

impl<T: Serialize + Carries<MethodBlacklistData>> Render for MethodBlacklistView<T> {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["METHOD"]);
        for m in &self.0.data().methods {
            t.add_row(vec![Cell::new(m)]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct RangeLimitsView<T>(T);

impl<T: Serialize + Carries<RangeLimitsData>> Render for RangeLimitsView<T> {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let data = self.0.data();
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["METHOD", "MAX_BLOCKS"]);
        let mut methods: Vec<&String> = data.limits.keys().collect();
        methods.sort();
        for m in methods {
            t.add_row(vec![Cell::new(m), Cell::new(data.limits[m])]);
        }
        write_table(w, &t)?;
        writeln!(
            w,
            "supported methods: {}",
            data.supported_methods.join(", ")
        )
    }
}

#[derive(Serialize)]
struct BlockedIpsView(quicknode_sdk::admin::GetBlockedIpsResponse);

impl Render for BlockedIpsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["ID", "IP"]);
        for i in &self.0.data {
            t.add_row(vec![Cell::new(&i.id), Cell::new(&i.ip)]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
struct BlockedIpCreatedView(quicknode_sdk::admin::CreateBlockedIpResponse);

impl Render for BlockedIpCreatedView {
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

#[cfg(test)]
mod tests {
    use super::parse_range_limit;

    #[test]
    fn parses_method_equals_blocks() {
        assert_eq!(
            parse_range_limit("eth_getLogs=1000").unwrap(),
            ("eth_getLogs".to_string(), 1000)
        );
        assert_eq!(
            parse_range_limit(" eth_getLogs = 5 ").unwrap(),
            ("eth_getLogs".to_string(), 5)
        );
    }

    #[test]
    fn rejects_malformed_pairs() {
        for bad in [
            "eth_getLogs",
            "=10",
            "eth_getLogs=0",
            "eth_getLogs=-1",
            "eth_getLogs=ten",
        ] {
            assert!(parse_range_limit(bad).is_err(), "{bad} should be rejected");
        }
    }
}
