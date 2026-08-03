//! `qn wallet …` — a local store of dedicated payment wallets.
//!
//! Removes the raw-key-file juggling the paid RPC lane otherwise requires:
//! `generate` creates a fresh keypair, stores the raw key at 0600 under
//! `<config-dir>/qn/wallets/<name>`, and prints the address (plus a QR on a
//! TTY) so the wallet can be funded. `qn rpc call --payment-wallet <name>`
//! then resolves the key by name — the raw key never appears on a command line.
//!
//! Storage layout, per wallet `<name>`:
//! - `<name>`        — the raw private key (0600), the exact bytes the paid
//!   lane's `read_key_file` expects.
//! - `<name>.toml`   — public metadata (vm, address, created-at). Never the
//!   key, so `list`/`show` read this and never open the key file.
//!
//! The key is stored **unencrypted** at 0600 (the `solana-keygen` model): the
//! paid lane is keyless and non-interactive, so a passphrase prompt would break
//! it. Treat every managed wallet as a dedicated, minimally-funded hot wallet.

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

// ── verbs ────────────────────────────────────────────────────────────────────

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

    // Key first (0600, tightens the dir to 0700), then the public sidecar.
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

    // Remove the key first: if the sidecar removal somehow fails, we never leave
    // an orphaned key on disk.
    if key_path.exists() {
        std::fs::remove_file(&key_path).map_err(|e| remove_err(&key_path, e))?;
    }
    if meta_path.exists() {
        std::fs::remove_file(&meta_path).map_err(|e| remove_err(&meta_path, e))?;
    }
    ctx.out.note(&format!("✓ Deleted wallet '{name}'"));
    Ok(())
}

// ── rendering ──────────────────────────────────────────────────────────────

/// The custody disclaimer shown on generate/show. This wallet lives only on
/// this machine; backing it up is the user's responsibility.
const CUSTODY_NOTE: &str = "This wallet is stored only on this machine. \
    Quicknode does not hold, back up, or recover it — keep your own backup of \
    the key file; if you lose it, any funds in the wallet are gone.";

/// Prints the address to stdout (the pipeable value), and to stderr a spaced,
/// lightly-styled block: the QR (TTY only), the private key file path, a
/// funding hint, and the custody note. Everything but the bare address goes to
/// stderr, so a piped `qn wallet show` yields just the address; the QR
/// must never go to stdout — it would corrupt the pipe. Styling is applied
/// only when `ctx.out.color` is set (a TTY with color enabled).
fn emit_address(ctx: &Ctx, meta: &WalletMeta, key_path: &Path, with_qr: bool) {
    println!("{}", meta.address);
    if ctx.out.quiet {
        return;
    }
    let c = ctx.out.color;
    let on_tty = with_qr && ctx.out.stdout_is_tty;

    // Build one spaced block so blank lines land predictably around the QR.
    let mut block = String::new();

    if on_tty {
        if let Some(qr) = render_qr(&meta.address) {
            // Blank line above so the QR isn't jammed against the address.
            block.push('\n');
            block.push_str(&qr);
            block.push('\n');
        }
    }

    // Private key file path (the address is already on stdout, so it isn't
    // echoed here).
    block.push_str(&format!(
        "{} {}\n",
        style("Private key file:", Style::Dim, c),
        style(&key_path.display().to_string(), Style::Bold, c)
    ));

    // Funding hint (only meaningful interactively, where the QR is shown).
    // Show the testnet funding routes for this wallet's VM family, then a
    // complete, runnable paid-lane call so the address can go straight from
    // funded to used.
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

    // Custody note as dim fine-print, set off by a blank line.
    block.push('\n');
    block.push_str(&style(&format!("⚠ {CUSTODY_NOTE}"), Style::Dim, c));

    ctx.out.note(&block);
}

/// A paste-ready `[rpc.payment]` config section that makes this wallet the
/// default payer, with network/asset/ceiling defaults matching the printed
/// example call. The section only supplies values — a scheme flag
/// (`--x402`/`--mpp`) still activates payment per call.
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

/// Testnet funding routes for a fresh wallet, per VM family. EVM gets the
/// gateway's built-in Base Sepolia faucet (`qn rpc x402 drip`, pre-filled with
/// this wallet's name), the Circle faucet for the USDC testnets, the Tempo
/// testnet faucet (pathUSD, for MPP), and the XLayer USDG note; SVM gets the
/// Circle faucet on Solana Devnet.
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

/// A complete, runnable paid-lane `qn rpc call` for a freshly funded wallet,
/// matching the documented examples in the README. SVM gets one x402/Solana
/// example; EVM gets both an x402 (pays Base Sepolia USDC) and an MPP (pays
/// Tempo testnet USDC) example, since the same secp256k1 key works for both.
/// The EVM examples query a chain the wallet is not funded on — the payment
/// chain is independent of the chain a call queries.
/// `--max-amount` is a spend ceiling in base units, not a tier selector: the
/// CLI pays the cheapest offer at or under it. The Solana example uses 1000000
/// (0.001 USDC at 6 decimals), which covers that gateway's per-request offer.
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

/// Unicode (half-block) QR of `data`, or `None` if it can't be encoded.
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

// ── metadata sidecar ─────────────────────────────────────────────────────────

/// Public wallet metadata, stored as `<name>.toml` beside the key. Never holds
/// the key itself, so `list`/`show` never open the 0600 key file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalletMeta {
    name: String,
    // `alias` keeps sidecars written before the `--vm` rename loadable.
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
    // The sidecar is public metadata, but reuse the atomic writer for the same
    // temp-file/rename durability (0600 is harmless for a public file).
    crate::config::write_atomic_0600(path, text.as_bytes(), ".qn-wallet-meta-")
}

fn load_meta(path: &Path) -> Option<WalletMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// Loads every `<name>.toml` sidecar in the wallets directory. Missing dir or
/// unreadable sidecars yield an empty list / are skipped — `list` is best-effort.
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

// ── paths & validation ─────────────────────────────────────────────────────

fn wallets_dir(ctx: &Ctx) -> Result<PathBuf, CliError> {
    let config_path = ctx.global.resolve_config_path();
    crate::config::wallets_dir(config_path.as_deref()).ok_or_else(|| {
        CliError::Arg("could not resolve a config directory for the wallet store".to_string())
    })
}

fn meta_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.toml"))
}

/// Restricts a wallet name to `[a-z0-9_-]` so it can never escape the wallets
/// directory (no separators, no `..`, no empties). Returns the validated name.
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

/// Resolves a stored wallet name to its key file path, validating the name and
/// checking the file exists. Also used by the paid RPC lane's
/// `--payment-wallet`, so the name rules live in one place.
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
