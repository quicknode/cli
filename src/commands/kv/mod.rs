//! `qn kv …` — KV store (sets and lists).

mod actions;
mod render;

use std::io::{IsTerminal, Read};

use clap::{Args as ClapArgs, Subcommand};

use crate::context::Ctx;
use crate::errors::CliError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: KvCmd,
}

#[derive(Debug, Subcommand)]
pub enum KvCmd {
    /// Sets: a single string value under a key.
    #[command(subcommand)]
    Set(SetCmd),
    /// Lists: ordered string collections under a key.
    #[command(subcommand)]
    List(ListCmd),
}

#[derive(Debug, Subcommand)]
pub enum SetCmd {
    /// Store a value under a key. Pass `-` as VALUE to read from stdin.
    Put { key: String, value: String },
    /// Get the value stored under a key.
    Get { key: String },
    /// List all key/value entries.
    Ls(SetsLsArgs),
    /// Delete a single set.
    Delete { key: String },
    /// Add and/or delete multiple sets in one call.
    Bulk(BulkArgs),
}

#[derive(Debug, ClapArgs)]
pub struct SetsLsArgs {
    #[arg(long)]
    pub limit: Option<i64>,
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct BulkArgs {
    /// Add a `KEY=VALUE` pair (repeatable).
    #[arg(long = "add")]
    pub add: Vec<String>,
    /// Delete a key (repeatable).
    #[arg(long = "delete")]
    pub delete: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum ListCmd {
    /// List all list keys.
    Ls(ListsLsArgs),
    /// Get items in a specific list (paginated).
    Get(ListGetArgs),
    /// Create a new list seeded with items.
    Create { key: String, items: Vec<String> },
    /// Append a single item to a list.
    Append { key: String, item: String },
    /// Check whether a list contains an item.
    Contains { key: String, item: String },
    /// Remove a single item from a list.
    RemoveItem { key: String, item: String },
    /// Add and/or remove items in a single call.
    Update(ListUpdateArgs),
    /// Delete a list (and all its items).
    Delete { key: String },
}

#[derive(Debug, ClapArgs)]
pub struct ListsLsArgs {
    #[arg(long)]
    pub limit: Option<i64>,
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct ListGetArgs {
    pub key: String,
    #[arg(long)]
    pub limit: Option<i64>,
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct ListUpdateArgs {
    pub key: String,
    /// Item to add (repeatable).
    #[arg(long = "add")]
    pub add_items: Vec<String>,
    /// Item to remove (repeatable).
    #[arg(long = "remove")]
    pub remove_items: Vec<String>,
}

pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError> {
    match args.cmd {
        KvCmd::Set(c) => actions::set(c, ctx).await,
        KvCmd::List(c) => actions::list(c, ctx).await,
    }
}

fn read_stdin() -> Result<String, CliError> {
    if std::io::stdin().is_terminal() {
        return Err(CliError::Arg(
            "value `-` requires stdin to be piped".to_string(),
        ));
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf.trim_end_matches('\n').to_string())
}
