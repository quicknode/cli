//! `qn agent context` — print an embedded, version-stamped usage guide for
//! AI agents and automated tools.
//!
//! This command builds no SDK and makes no network call: an agent will often
//! run it *before* it has a key, so it must never trigger the API-key path.
//! Dispatched directly from `GlobalArgs` in `cli.rs`, alongside `auth` and
//! `completions`.
//!
//! The guide lives in `context.md` (embedded via `include_str!`) and carries a
//! `{{VERSION}}` placeholder filled in at print time. See the sync rule in
//! CLAUDE.md: the prose makes version-stamped claims about the command surface,
//! so any change to that surface must update `context.md` in the same commit.

use std::io::Write;

use clap::{Args as ClapArgs, Subcommand};
use serde::Serialize;

use crate::context::GlobalArgs;
use crate::errors::CliError;
use crate::output::Format;

/// The embedded guide. `{{VERSION}}` is replaced with the crate version at print time.
const GUIDE: &str = include_str!("context.md");

#[derive(Debug, ClapArgs)]
#[command(
    about = "Resources for AI agents and automated tools.",
    long_about = "Resources for AI agents and automated tools.\n\n\
                  Run `qn agent context` for a single, self-contained usage guide\n\
                  (auth, output formats, exit codes, confirmation, retry/idempotency,\n\
                  the command catalog, and common workflows)."
)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: AgentCmd,
}

#[derive(Debug, Subcommand)]
pub enum AgentCmd {
    /// Print a machine-readable usage guide for agents (no auth, no network).
    Context,
}

pub async fn run(args: Args, global: GlobalArgs) -> Result<(), CliError> {
    match args.cmd {
        AgentCmd::Context => context(global),
    }
}

/// JSON envelope for `-o json`: the guide stays self-describing even after the
/// `guide` string is extracted.
#[derive(Serialize)]
struct ContextView<'a> {
    version: &'a str,
    guide: &'a str,
}

fn context(global: GlobalArgs) -> Result<(), CliError> {
    let version = env!("CARGO_PKG_VERSION");
    let guide = GUIDE.replace("{{VERSION}}", version);

    // Read `global.format` directly (the raw Option), NOT `resolve_format` —
    // here `None` means "print markdown", not the piped-default TOON. This
    // command's stdout is prose to read, not data to parse.
    match global.format {
        Some(Format::Json) => {
            let view = ContextView {
                version,
                guide: &guide,
            };
            let mut out = std::io::stdout().lock();
            serde_json::to_writer_pretty(&mut out, &view)?;
            writeln!(out)?;
        }
        other => {
            print!("{guide}");
            // An explicit non-markdown format (yaml/toon/table) can't carry the
            // guide usefully, so we print markdown and say why on stderr.
            if matches!(other, Some(Format::Yaml | Format::Toon | Format::Table)) && !global.quiet {
                let fmt = match other {
                    Some(Format::Yaml) => "yaml",
                    Some(Format::Toon) => "toon",
                    _ => "table",
                };
                let _ = writeln!(
                    std::io::stderr(),
                    "ℹ '-o {fmt}' isn't supported by 'qn agent context'; printing markdown. Use '-o json' for structured output."
                );
            }
        }
    }
    Ok(())
}
