//! Local payment-wallet storage. Raw keys are unencrypted files with 0600
//! permissions; public metadata is stored in a separate sidecar.

use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use comfy_table::Cell;
use quicknode_sdk::{generate_payment_wallet, ChainKind};
use serde::{Deserialize, Serialize};

use crate::confirm::confirm_mild;
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, set_header_bold, style, write_table, Render, Style};

#[derive(Debug, ClapArgs)]
#[command(subcommand_required = true, arg_required_else_help = true)]
#[command(after_help = "Examples:\n  \
    qn wallet generate --vm evm --name payer\n  \
    qn wallet list\n  \
    qn wallet show payer\n  \
    qn wallet rm payer")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: WalletCmd,
}

#[derive(Debug, Subcommand)]
pub enum WalletCmd {
    /// Generate a new payment wallet and store it locally.
    #[command(after_help = "Examples:\n  \
        qn wallet generate --vm evm --name payer\n  \
        qn wallet generate --vm svm --name sol-payer")]
    Generate(GenerateArgs),

    /// List stored wallets (names, VM family, address — never keys).
    #[command(visible_alias = "ls")]
    List,

    /// Show a wallet's address, with a QR to fund it (on a terminal).
    #[command(after_help = "Examples:\n  \
        qn wallet show payer")]
    Show(ShowArgs),

    /// Delete a stored wallet. The key is the only copy and cannot be recovered.
    #[command(after_help = "Examples:\n  \
        qn wallet rm payer\n  \
        qn wallet rm payer --yes")]
    Rm(RmArgs),
}

/// VM family a generated wallet targets. `evm` also covers MPP/Tempo
/// (same secp256k1 key format).
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lower")]
pub enum WalletVm {
    /// secp256k1 wallet for x402/EVM and MPP/Tempo (`0x…` hex key).
    Evm,
    /// ed25519 wallet for x402/Solana (base58 key).
    Svm,
}

impl WalletVm {
    fn kind(self) -> ChainKind {
        match self {
            WalletVm::Evm => ChainKind::Evm,
            WalletVm::Svm => ChainKind::Svm,
        }
    }

    fn label(self) -> &'static str {
        match self {
            WalletVm::Evm => "evm",
            WalletVm::Svm => "svm",
        }
    }
}

#[derive(Debug, ClapArgs)]
pub struct GenerateArgs {
    /// VM family: `evm` (x402/EVM, also MPP/Tempo) or `svm` (x402/Solana).
    #[arg(long)]
    pub vm: WalletVm,

    /// Wallet name (a-z, 0-9, `-`, `_`). Becomes the key file name and the
    /// `--payment-wallet` handle.
    #[arg(long, value_name = "NAME")]
    pub name: String,

    /// Overwrite an existing wallet of the same name.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, ClapArgs)]
pub struct ShowArgs {
    /// Wallet name.
    #[arg(value_name = "NAME")]
    pub name: String,
}

#[derive(Debug, ClapArgs)]
pub struct RmArgs {
    /// Wallet name.
    #[arg(value_name = "NAME")]
    pub name: String,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        WalletCmd::Generate(a) => generate(a, ctx),
        WalletCmd::List => list(ctx),
        WalletCmd::Show(a) => show(a, ctx),
        WalletCmd::Rm(a) => rm(a, ctx),
    }
}

fn generate(a: GenerateArgs, ctx: Ctx) -> Result<(), CliError> {
    let name = validate_name(&a.name)?;
    let dir = wallets_dir(&ctx)?;
    let key_path = dir.join(&name);
    let meta_path = meta_path(&dir, &name);

    if !a.force && (key_path.exists() || meta_path.exists()) {
        return Err(CliError::Arg(format!(
            "wallet '{name}' already exists. Pass --force to overwrite, or pick another name"
        )));
    }

    let wallet = generate_payment_wallet(a.vm.kind())?;
    let meta = WalletMeta {
        name: name.clone(),
        vm: a.vm.label().to_string(),
        address: wallet.address.clone(),
        created_at_unix: now_unix(),
    };
    let raw = wallet.into_key();

    // Write the key before its metadata so a partial write leaves no usable record.
    crate::config::write_atomic_0600(&key_path, raw.as_bytes(), ".qn-wallet-")?;
    write_meta(&meta_path, &meta)?;

    ctx.out
        .note(&format!("✓ Generated {} wallet '{name}'", a.vm.label()));
    emit_address(&ctx, &meta, &key_path, /* with_qr */ true);
    Ok(())
}

fn list(ctx: Ctx) -> Result<(), CliError> {
    let dir = wallets_dir(&ctx)?;
    let mut wallets = load_all_meta(&dir);
    wallets.sort_by(|a, b| a.name.cmp(&b.name));
    crate::output::emit(&ctx.out, &WalletsView(wallets))
}

fn show(a: ShowArgs, ctx: Ctx) -> Result<(), CliError> {
    let name = validate_name(&a.name)?;
    let dir = wallets_dir(&ctx)?;
    let meta = load_meta(&meta_path(&dir, &name)).ok_or_else(|| not_found(&name))?;

    if ctx.out.format.is_structured() {
        return crate::output::emit(&ctx.out, &meta);
    }
    emit_address(&ctx, &meta, &dir.join(&name), /* with_qr */ true);
    Ok(())
}

fn rm(a: RmArgs, ctx: Ctx) -> Result<(), CliError> {
    let name = validate_name(&a.name)?;
    let dir = wallets_dir(&ctx)?;
    let key_path = dir.join(&name);
    let meta_path = meta_path(&dir, &name);
    if !key_path.exists() && !meta_path.exists() {
        return Err(not_found(&name));
    }

    confirm_mild(
        &ctx,
        &format!(
            "Delete wallet '{name}'? The private key is destroyed locally; without a backup \
             of the key, funds at its address are unrecoverable."
        ),
    )?;

    // Remove the key first; a leftover sidecar must not preserve a usable key.
    if key_path.exists() {
        std::fs::remove_file(&key_path).map_err(|e| remove_err(&key_path, e))?;
    }
    if meta_path.exists() {
        std::fs::remove_file(&meta_path).map_err(|e| remove_err(&meta_path, e))?;
    }
    ctx.out.note(&format!("✓ Deleted wallet '{name}'"));
    Ok(())
}

// The custody disclaimer shown on generate/show.
const CUSTODY_NOTE: &str = "This wallet is stored only on this machine. \
    Quicknode does not hold, back up, or recover it — keep your own backup of \
    the key file; if you lose it, any funds in the wallet are gone.";

/// Print the address to stdout and interactive details to stderr.
fn emit_address(ctx: &Ctx, meta: &WalletMeta, key_path: &Path, with_qr: bool) {
    println!("{}", meta.address);
    if ctx.out.quiet {
        return;
    }
    let c = ctx.out.color;
    let on_tty = with_qr && ctx.out.stdout_is_tty;

    let mut block = String::new();

    if on_tty {
        if let Some(qr) = render_qr(&meta.address) {
            block.push('\n');
            block.push_str(&qr);
            block.push('\n');
        }
    }

    block.push_str(&format!(
        "{} {}\n",
        style("Private key file:", Style::Dim, c),
        style(&key_path.display().to_string(), Style::Bold, c)
    ));

    if on_tty {
        block.push('\n');
        block.push_str(
            "This wallet can be used to pay for RPC calls via micropayments (x402/MPP).\n\
             Fund it on the network you'll pay from.\n\nTestnet funding:\n\n",
        );
        block.push_str(&format!("{}\n\n", funding_hint(&meta.vm, &meta.name, c)));
        block.push_str("Then use it:\n\n");
        block.push_str(&format!(
            "{}\n",
            style(&example_call(&meta.vm, &meta.name), Style::Bold, c),
        ));

        let config_path = ctx
            .global
            .resolve_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.config/qn/config.toml".to_string());
        block.push('\n');
        block.push_str(&format!(
            "To make it the default payment wallet (per-call flags still \
             override), add to {config_path}:\n\n",
        ));
        block.push_str(&format!("{}\n", config_example(&meta.vm, &meta.name)));
    }

    block.push('\n');
    block.push_str(&style(&format!("⚠ {CUSTODY_NOTE}"), Style::Dim, c));

    ctx.out.note(&block);
}

// Build the matching `[rpc.payment]` example.
fn config_example(vm: &str, name: &str) -> String {
    let network = match vm {
        "svm" => "solana-devnet",
        _ => "base-sepolia",
    };
    format!(
        "  [rpc.payment]\n  \
           wallet = \"{name}\"\n  \
           payment_network = \"{network}\"\n  \
           payment_asset = \"USDC\"\n  \
           max_amount = 1000"
    )
}

// Build testnet funding hints for the wallet VM.
fn funding_hint(vm: &str, name: &str, color: bool) -> String {
    match vm {
        "svm" => "  Circle faucet:  https://faucet.circle.com (USDC on Solana Devnet)".to_string(),
        _ => {
            let drip = style(
                &format!("qn rpc x402 drip --payment-wallet {name} --payment-network base-sepolia"),
                Style::Bold,
                color,
            );
            format!(
                "  Base Sepolia:   {drip}\n  \
                   Circle faucet:  https://faucet.circle.com (USDC on Base Sepolia, Polygon Amoy, Arc)\n  \
                   Tempo Testnet:  https://tempo.xyz/developers/docs/quickstart/faucet (pathUSD, for MPP)\n  \
                   XLayer Testnet: send USDG to the address above"
            )
        }
    }
}

// Build a runnable paid call for a freshly funded wallet.
fn example_call(vm: &str, name: &str) -> String {
    match vm {
        "svm" => format!(
            "qn rpc call getSlot \\\n  \
             --network solana-devnet \\\n  \
             --x402 \\\n  \
             --payment-wallet {name} \\\n  \
             --payment-network solana-devnet \\\n  \
             --payment-asset USDC \\\n  \
             --max-amount 1000000"
        ),
        _ => format!(
            "# x402 (pays Base Sepolia USDC):\n\
             qn rpc call eth_blockNumber \\\n  \
             --network ethereum-mainnet \\\n  \
             --x402 \\\n  \
             --payment-wallet {name} \\\n  \
             --payment-network base-sepolia \\\n  \
             --payment-asset USDC \\\n  \
             --max-amount 1000\n\n\
             # MPP (pays Tempo testnet USDC):\n\
             qn rpc call eth_blockNumber \\\n  \
             --network ethereum-mainnet \\\n  \
             --mpp \\\n  \
             --payment-wallet {name} \\\n  \
             --payment-network tempo-testnet \\\n  \
             --payment-asset USDC \\\n  \
             --max-amount 1000"
        ),
    }
}

/// Render a Unicode QR code, if encoding succeeds.
fn render_qr(data: &str) -> Option<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(data.as_bytes()).ok()?;
    Some(
        code.render::<unicode::Dense1x2>()
            .dark_color(unicode::Dense1x2::Light)
            .light_color(unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

#[derive(Serialize)]
struct WalletsView(Vec<WalletMeta>);

impl Render for WalletsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        if self.0.is_empty() {
            writeln!(w, "(no wallets)")?;
            return Ok(());
        }
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["NAME", "VM", "ADDRESS"]);
        for wl in &self.0 {
            t.add_row(vec![
                Cell::new(&wl.name),
                Cell::new(&wl.vm),
                Cell::new(&wl.address),
            ]);
        }
        write_table(w, &t)
    }
}

// Public wallet metadata stored beside the key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalletMeta {
    name: String,
    #[serde(alias = "chain")]
    vm: String,
    address: String,
    created_at_unix: i64,
}

impl Render for WalletMeta {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["NAME", "VM", "ADDRESS"]);
        t.add_row(vec![
            Cell::new(&self.name),
            Cell::new(&self.vm),
            Cell::new(&self.address),
        ]);
        write_table(w, &t)
    }
}

fn write_meta(path: &Path, meta: &WalletMeta) -> Result<(), CliError> {
    let text = toml::to_string_pretty(meta).map_err(|e| CliError::ConfigWrite {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;
    crate::config::write_atomic_0600(path, text.as_bytes(), ".qn-wallet-meta-")
}

fn load_meta(path: &Path) -> Option<WalletMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// Load readable wallet metadata sidecars.
fn load_all_meta(dir: &Path) -> Vec<WalletMeta> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| load_meta(&e.path()))
        .collect()
}

fn wallets_dir(ctx: &Ctx) -> Result<PathBuf, CliError> {
    let config_path = ctx.global.resolve_config_path();
    crate::config::wallets_dir(config_path.as_deref()).ok_or_else(|| {
        CliError::Arg("could not resolve a config directory for the wallet store".to_string())
    })
}

fn meta_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.toml"))
}

/// Validate a wallet name for use as a filename.
fn validate_name(name: &str) -> Result<String, CliError> {
    if name.is_empty() {
        return Err(CliError::Arg("wallet name cannot be empty".to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(CliError::Arg(format!(
            "invalid wallet name '{name}'. Use lowercase letters, digits, '-' and '_' only"
        )));
    }
    Ok(name.to_string())
}

fn not_found(name: &str) -> CliError {
    CliError::Arg(format!(
        "no wallet named '{name}'. Run 'qn wallet list' to see stored wallets, \
         or create one with 'qn wallet generate'"
    ))
}

/// Resolve a stored wallet name to its key file.
pub(crate) fn key_path(name: &str, wallets_dir: Option<&Path>) -> Result<PathBuf, CliError> {
    let name = validate_name(name)?;
    let dir = wallets_dir
        .ok_or_else(|| CliError::Arg("could not resolve the wallet store directory".to_string()))?;
    let path = dir.join(&name);
    if !path.exists() {
        return Err(not_found(&name));
    }
    Ok(path)
}

fn remove_err(path: &Path, source: std::io::Error) -> CliError {
    CliError::ConfigWrite {
        path: path.to_path_buf(),
        source,
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
