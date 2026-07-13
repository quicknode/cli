//! Runtime context shared by every command.
//!
//! Builds the `QuicknodeSdk` from `GlobalArgs` and attaches an `OutputCtx`.

use std::io::IsTerminal;

use quicknode_sdk::{
    AdminConfig, CachedToken, HttpConfig, KvStoreConfig, QuicknodeSdk, RpcConfig, SdkFullConfig,
    SqlConfig, StreamsConfig, WebhooksConfig,
};

use crate::config;
use crate::errors::CliError;
use crate::output::{Format, OutputCtx};

/// Top-level flags inherited by every subcommand.
#[derive(Debug, Clone, Default)]
pub struct GlobalArgs {
    pub api_key: Option<String>,
    /// `--config-file`: alternate config TOML. `None` means the default path
    /// (`~/.config/qn/config.toml`).
    pub config_file: Option<std::path::PathBuf>,
    /// `None` means the user didn't pass `--format`; resolve via config file
    /// (then the TTY-aware default: `Table` on a TTY, `Json` off) when we
    /// build the [`Ctx`].
    pub format: Option<Format>,
    pub wide: bool,
    pub no_color: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub no_input: bool,
    pub yes_count: u8,
    /// Max automatic retries for read-only API calls (see `crate::retry`).
    /// `Default` yields 0 (no retries) — the CLI default of 3 comes from clap.
    pub retries: u32,
    pub base_url: Option<String>,
    /// Optional path prefix inserted between the host and each sub-client's
    /// fixed suffix (e.g. `/console-api`). Requires `base_url`. Useful for
    /// reverse-proxy / gateway environments and local servers that mount the
    /// API under a prefix.
    pub base_prefix: Option<String>,
}

impl GlobalArgs {
    /// Resolve the output format: CLI flag > config file > TTY-aware default
    /// (`Table` on a TTY, `Json` off).
    /// Used by [`Ctx::from_global`] and `auth` (which doesn't build a Ctx).
    pub fn resolve_format(&self, stdout_is_tty: bool) -> Format {
        self.resolve_output(stdout_is_tty).0
    }

    /// Resolve `(format, wide)` together so we only read the config file once.
    ///
    /// For each: CLI flag > config file > built-in default. The format default
    /// is TTY-aware: `Table` when stdout is a terminal, `Json` otherwise (so
    /// agents / piped callers get a structured format by default). `--wide` is
    /// purely additive — the flag sets it true; the config file can also set
    /// it true; otherwise it's false.
    pub fn resolve_output(&self, stdout_is_tty: bool) -> (Format, bool) {
        let (cfg_format, cfg_wide) = self.load_output_config();
        resolve_output_inner(self.format, self.wide, cfg_format, cfg_wide, stdout_is_tty)
    }

    /// The config file to read: `--config-file` if given, else the default path.
    pub fn resolve_config_path(&self) -> Option<std::path::PathBuf> {
        self.config_file.clone().or_else(config::config_path)
    }

    fn load_output_config(&self) -> (Option<Format>, bool) {
        let Some(p) = self.resolve_config_path() else {
            return (None, false);
        };
        match config::load_from(&p) {
            Ok(Some(cfg)) => (cfg.output.format, cfg.output.wide),
            _ => (None, false),
        }
    }
}

/// Pure form of [`GlobalArgs::resolve_output`] — separated so it can be
/// exhaustively unit-tested without touching the real config file. CLI values
/// win; otherwise we fall back to config; otherwise the TTY-aware default.
fn resolve_output_inner(
    flag_format: Option<Format>,
    flag_wide: bool,
    cfg_format: Option<Format>,
    cfg_wide: bool,
    stdout_is_tty: bool,
) -> (Format, bool) {
    let format = flag_format.or(cfg_format).unwrap_or(if stdout_is_tty {
        Format::Table
    } else {
        Format::Json
    });
    let wide = flag_wide || cfg_wide;
    (format, wide)
}

/// The `User-Agent` sent with every API request. Mirrors the SDK's own shape
/// (`quicknode-sdk-<lang>/<ver> (<os>-<arch>; …)`) with the CLI as the product:
/// `quicknode-cli/<version> (<os>-<arch>)`.
pub fn user_agent() -> String {
    format!(
        "quicknode-cli/{} ({}-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// Base SDK config shared by every construction site (`Ctx::from_global` and
/// the `auth` commands): the API key plus the CLI `User-Agent`. Custom headers
/// in `HttpConfig` override SDK-managed headers of the same name, which is the
/// SDK's supported way to replace its auto-generated `User-Agent`.
pub fn sdk_config(api_key: String) -> SdkFullConfig {
    let mut full = SdkFullConfig::from_api_key(api_key);
    apply_user_agent(&mut full);
    full
}

/// Installs the CLI `User-Agent` header on `full`. Shared by the keyed
/// ([`sdk_config`]) and keyless ([`Ctx::from_global_keyless_payment`])
/// construction paths.
fn apply_user_agent(full: &mut SdkFullConfig) {
    let mut headers = std::collections::HashMap::new();
    headers.insert("User-Agent".to_string(), user_agent());
    full.http = Some(HttpConfig {
        headers: Some(headers),
        ..Default::default()
    });
}

/// Points every sub-client at a custom host, suffixing each with its own base
/// path. Shared by `Ctx::from_global` and the `auth` commands so `--base-url`
/// applies uniformly (useful for wiremock tests and on-prem mirrors).
fn apply_base_url(full: &mut SdkFullConfig, trimmed: &str) {
    full.admin = Some(AdminConfig {
        base_url: Some(format!("{trimmed}/v0/")),
    });
    full.streams = Some(StreamsConfig {
        base_url: Some(format!("{trimmed}/streams/rest/v1/")),
    });
    full.webhooks = Some(WebhooksConfig {
        base_url: Some(format!("{trimmed}/webhooks/rest/v1/")),
    });
    full.kvstore = Some(KvStoreConfig {
        base_url: Some(format!("{trimmed}/kv/rest/v1/")),
    });
    full.sql = Some(SqlConfig {
        base_url: Some(format!("{trimmed}/sql/rest/v1/")),
    });
}

/// Like [`sdk_config`] but also honors an optional `--base-url` override.
/// Used by the `auth` commands, which build the SDK outside [`Ctx`].
pub fn sdk_config_with_base(
    api_key: String,
    base_url: Option<&str>,
) -> Result<SdkFullConfig, CliError> {
    let mut full = sdk_config(api_key);
    if let Some(base) = base_url {
        let trimmed = validate_base_url(base)?;
        apply_base_url(&mut full, trimmed.as_str());
    }
    Ok(full)
}

pub struct Ctx {
    pub sdk: QuicknodeSdk,
    pub out: OutputCtx,
    pub global: GlobalArgs,
}

impl Ctx {
    /// Construct the SDK + output ctx from `global`. Resolves the API key per
    /// the documented precedence: flag > config file (`--config-file` path if
    /// given, else the default). If neither supplies a key we return
    /// `CliError::NoApiKey` — regular commands do not prompt; the user is
    /// directed to `qn auth login`.
    pub fn from_global(global: GlobalArgs) -> Result<Self, CliError> {
        Self::build(global, None, None).map(|(ctx, _)| ctx)
    }

    /// Like [`from_global`](Self::from_global) but for `qn rpc`: seeds the RPC
    /// client's token cache with `seed` (a JWT loaded from disk) and sets the
    /// client-wide custom endpoint URL from `config_endpoint_url` (the
    /// `[rpc] endpoint_url` config default). Also returns the resolved API key
    /// so the caller can scope and write back the token cache.
    pub fn from_global_with_rpc_seed(
        global: GlobalArgs,
        seed: Option<CachedToken>,
        config_endpoint_url: Option<String>,
    ) -> Result<(Self, String), CliError> {
        Self::build(global, seed, config_endpoint_url)
    }

    /// Keyless construction for the crypto-micropayment lane of `qn rpc call`
    /// (`--x402`/`--mpp`): no API key is resolved or required, so it works on
    /// a machine that has never run `qn auth login`. Only the RPC payment lane
    /// is usable — every keyed sub-client would 401.
    ///
    /// Deliberately NOT applied here:
    /// - the token cache seed and `[rpc] endpoint_url` (either would conflict
    ///   with the payment lane — the SDK rejects a custom URL + payment);
    /// - `--base-url` sub-client overrides (the paid lane's test hook rides in
    ///   `PaymentConfig.base_url_override`, set by the caller; no control-plane
    ///   sub-client is used).
    pub fn from_global_keyless_payment(
        global: GlobalArgs,
        payment: quicknode_sdk::PaymentConfig,
    ) -> Result<Self, CliError> {
        let stdout_is_tty = std::io::stdout().is_terminal();
        let (format, wide) = global.resolve_output(stdout_is_tty);

        let mut full = SdkFullConfig::keyless();
        apply_user_agent(&mut full);
        full.rpc = Some(RpcConfig {
            payment: Some(payment),
            ..Default::default()
        });

        let sdk = QuicknodeSdk::new(&full)?;
        let out = OutputCtx::detect_with(
            format,
            global.no_color,
            global.quiet,
            global.verbose,
            wide,
            stdout_is_tty,
            std::env::var_os("NO_COLOR"),
            std::env::var("TERM").ok(),
        );

        Ok(Self { sdk, out, global })
    }

    fn build(
        global: GlobalArgs,
        rpc_seed: Option<CachedToken>,
        rpc_endpoint_url: Option<String>,
    ) -> Result<(Self, String), CliError> {
        let config_path = global.resolve_config_path();
        let stdout_is_tty = std::io::stdout().is_terminal();
        let (format, wide) = global.resolve_output(stdout_is_tty);

        let (api_key, _) = config::resolve_api_key(
            global.api_key.as_deref(),
            config_path.as_deref(),
            false,
            || unreachable!("prompt disabled for non-auth commands"),
        )?;

        let mut full = sdk_config(api_key.clone());

        // The `[rpc] endpoint_url` config default becomes the client-wide custom
        // URL (a per-call `--endpoint-url` overrides it in the call itself). We
        // validate it here so a malformed config value fails with a clear error
        // rather than at call time. `seed` and `endpoint_url` coexist harmlessly:
        // the SDK ignores the seed when a custom URL is set.
        let rpc_endpoint_url = match rpc_endpoint_url {
            Some(u) => Some(validate_endpoint_url(&u)?),
            None => None,
        };
        if rpc_seed.is_some() || rpc_endpoint_url.is_some() {
            full.rpc = Some(RpcConfig {
                seed: rpc_seed,
                endpoint_url: rpc_endpoint_url,
                ..Default::default()
            });
        }

        // --base-prefix only makes sense when overriding the host. Composing it
        // against the default prod host isn't supported, so fail loudly rather
        // than silently ignore it.
        if global.base_prefix.is_some() && global.base_url.is_none() {
            return Err(CliError::Arg(
                "--base-prefix requires --base-url".to_string(),
            ));
        }

        // --base-url applies to every sub-client. Useful for wiremock tests and
        // on-prem mirrors. Each sub-client has its own fixed suffix; an optional
        // --base-prefix is inserted between the host and that suffix for
        // reverse-proxy / gateway environments. Tooling Access / RPC minting
        // lives on the admin `v0` base, so no separate RPC base is needed here.
        if let Some(base) = &global.base_url {
            let host = validate_base_url(base)?;
            let prefix = match &global.base_prefix {
                Some(p) => validate_base_prefix(p)?,
                None => String::new(),
            };
            let root = format!("{host}{prefix}");
            apply_base_url(&mut full, &root);
        }

        let sdk = QuicknodeSdk::new(&full)?;
        let out = OutputCtx::detect_with(
            format,
            global.no_color,
            global.quiet,
            global.verbose,
            wide,
            stdout_is_tty,
            std::env::var_os("NO_COLOR"),
            std::env::var("TERM").ok(),
        );

        Ok((Self { sdk, out, global }, api_key))
    }
}

/// Validates a user-supplied `--base-url` and returns it with any trailing
/// slash stripped. Rejects non-http(s) schemes, embedded userinfo, query/
/// fragment, and non-root paths so we can't accidentally splice attacker-
/// controlled segments into the SDK's hard-coded sub-client paths.
fn validate_base_url(base: &str) -> Result<String, CliError> {
    let parsed = url::Url::parse(base)
        .map_err(|_| CliError::Arg(format!("--base-url '{base}' is not a valid URL")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CliError::Arg(format!(
                "--base-url scheme '{other}' is not allowed; use http or https"
            )))
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CliError::Arg(
            "--base-url must not contain userinfo (username/password)".into(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(CliError::Arg(
            "--base-url must not contain a query string or fragment".into(),
        ));
    }
    if !matches!(parsed.path(), "" | "/") {
        return Err(CliError::Arg("--base-url must not contain a path".into()));
    }
    Ok(base.trim_end_matches('/').to_string())
}

/// Validates a custom RPC endpoint URL (`--endpoint-url` or `[rpc] endpoint_url`).
/// Unlike [`validate_base_url`], a fully-formed RPC URL carries a path
/// (`https://host/rpc`), so paths are allowed here. We only confirm it parses
/// and uses an http(s) scheme — enough to reject garbage and non-network schemes
/// with a clear CLI error before any call, while leaving the rest to the SDK.
pub(crate) fn validate_endpoint_url(url: &str) -> Result<String, CliError> {
    let parsed = url::Url::parse(url)
        .map_err(|_| CliError::Arg(format!("--endpoint-url '{url}' is not a valid URL")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(url.to_string()),
        other => Err(CliError::Arg(format!(
            "--endpoint-url scheme '{other}' is not allowed; use http or https"
        ))),
    }
}

/// Validates and normalizes a `--base-prefix` to a leading-slash, no-trailing-
/// slash path fragment (e.g. `/console-api`). Rejects anything that smuggles in
/// a host or query so it can only ever extend the path of `--base-url`: no
/// scheme/authority (`//`), no `?`/`#`, no `.`/`..` traversal segments.
fn validate_base_prefix(prefix: &str) -> Result<String, CliError> {
    let trimmed = prefix.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains("//") {
        return Err(CliError::Arg(
            "--base-prefix must be a path, not a URL (no '//')".into(),
        ));
    }
    if trimmed.contains(['?', '#', '\\']) {
        return Err(CliError::Arg(
            "--base-prefix must not contain a query string, fragment, or backslash".into(),
        ));
    }
    let inner = trimmed.trim_matches('/');
    if inner.is_empty() {
        // Bare "/" (or "///") carries no prefix.
        return Ok(String::new());
    }
    let normalized = format!("/{inner}");
    if normalized.split('/').any(|seg| matches!(seg, "." | "..")) {
        return Err(CliError::Arg(
            "--base-prefix must not contain '.' or '..' path segments".into(),
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_format_wins_over_config_and_tty_default() {
        let (f, _) =
            resolve_output_inner(Some(Format::Json), false, Some(Format::Yaml), false, true);
        assert_eq!(f, Format::Json);
        let (f, _) =
            resolve_output_inner(Some(Format::Json), false, Some(Format::Yaml), false, false);
        assert_eq!(f, Format::Json);
    }

    #[test]
    fn config_format_wins_over_tty_default() {
        let (f, _) = resolve_output_inner(None, false, Some(Format::Yaml), false, true);
        assert_eq!(f, Format::Yaml);
        let (f, _) = resolve_output_inner(None, false, Some(Format::Yaml), false, false);
        assert_eq!(f, Format::Yaml);
    }

    #[test]
    fn default_is_table_when_stdout_is_a_tty() {
        let (f, _) = resolve_output_inner(None, false, None, false, true);
        assert_eq!(f, Format::Table);
    }

    #[test]
    fn default_is_json_when_stdout_is_not_a_tty() {
        let (f, _) = resolve_output_inner(None, false, None, false, false);
        assert_eq!(f, Format::Json);
    }

    #[test]
    fn config_toon_overrides_non_tty_default() {
        let (f, _) = resolve_output_inner(None, false, Some(Format::Toon), false, false);
        assert_eq!(f, Format::Toon);
    }

    #[test]
    fn wide_is_additive_between_flag_and_config() {
        // Flag alone.
        let (_, w) = resolve_output_inner(None, true, None, false, true);
        assert!(w);
        // Config alone.
        let (_, w) = resolve_output_inner(None, false, None, true, true);
        assert!(w);
        // Both.
        let (_, w) = resolve_output_inner(None, true, None, true, true);
        assert!(w);
        // Neither.
        let (_, w) = resolve_output_inner(None, false, None, false, true);
        assert!(!w);
    }

    #[test]
    fn base_url_accepts_plain_http_and_https() {
        assert_eq!(
            validate_base_url("https://api.quicknode.com").unwrap(),
            "https://api.quicknode.com"
        );
        assert_eq!(
            validate_base_url("http://127.0.0.1:8080/").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn base_url_rejects_non_http_schemes() {
        for bad in ["file:///etc/passwd", "ftp://x", "javascript:alert(1)"] {
            assert!(validate_base_url(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn base_url_rejects_userinfo() {
        assert!(validate_base_url("https://user:pass@evil/").is_err());
        assert!(validate_base_url("https://user@evil/").is_err());
    }

    #[test]
    fn base_url_rejects_path_query_fragment() {
        assert!(validate_base_url("https://x/extra/path").is_err());
        assert!(validate_base_url("https://x/?q=1").is_err());
        assert!(validate_base_url("https://x/#frag").is_err());
    }

    #[test]
    fn base_url_rejects_garbage() {
        assert!(validate_base_url("not a url").is_err());
        assert!(validate_base_url("").is_err());
    }

    #[test]
    fn endpoint_url_allows_http_https_with_path() {
        assert_eq!(
            validate_endpoint_url("https://my-endpoint.example/rpc").unwrap(),
            "https://my-endpoint.example/rpc"
        );
        assert_eq!(
            validate_endpoint_url("http://127.0.0.1:8080/some/path?x=1").unwrap(),
            "http://127.0.0.1:8080/some/path?x=1"
        );
    }

    #[test]
    fn endpoint_url_rejects_non_http_schemes_and_garbage() {
        for bad in ["ftp://x/rpc", "file:///etc/passwd", "not a url", ""] {
            assert!(validate_endpoint_url(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn base_prefix_normalizes_slashes() {
        assert_eq!(
            validate_base_prefix("/console-api").unwrap(),
            "/console-api"
        );
        assert_eq!(validate_base_prefix("console-api").unwrap(), "/console-api");
        assert_eq!(
            validate_base_prefix("/console-api/").unwrap(),
            "/console-api"
        );
        assert_eq!(validate_base_prefix("/a/b").unwrap(), "/a/b");
    }

    #[test]
    fn base_prefix_empty_is_empty() {
        assert_eq!(validate_base_prefix("").unwrap(), "");
        assert_eq!(validate_base_prefix("  ").unwrap(), "");
        assert_eq!(validate_base_prefix("/").unwrap(), "");
    }

    #[test]
    fn base_prefix_rejects_url_like_and_traversal() {
        assert!(validate_base_prefix("//evil.com").is_err());
        assert!(validate_base_prefix("http://evil.com").is_err());
        assert!(validate_base_prefix("/a?b=1").is_err());
        assert!(validate_base_prefix("/a#frag").is_err());
        assert!(validate_base_prefix("/../etc").is_err());
        assert!(validate_base_prefix("/a/../b").is_err());
    }

    #[test]
    fn user_agent_identifies_the_cli() {
        let ua = user_agent();
        assert!(ua.starts_with("quicknode-cli/"), "ua={ua}");
        assert!(ua.contains(env!("CARGO_PKG_VERSION")), "ua={ua}");
    }

    #[test]
    fn sdk_config_sets_the_user_agent_header_and_nothing_else() {
        let cfg = sdk_config("k".to_string());
        let http = cfg.http.expect("http config should be set");
        assert_eq!(
            http.headers.as_ref().and_then(|h| h.get("User-Agent")),
            Some(&user_agent())
        );
        // SDK defaults (timeout, pooling) must stay untouched.
        assert_eq!(http.timeout_secs, None);
        assert_eq!(http.pool_max_idle_per_host, None);
    }
}
