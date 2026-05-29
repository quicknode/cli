//! `qn kv …` — KV store (sets and lists).

use std::collections::HashMap;
use std::io::{IsTerminal, Read};

use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use quicknode_sdk::{
    AddListItemParams, BulkSetsParams, CreateListParams, CreateSetParams, GetListParams,
    GetListsParams, GetSetsParams, UpdateListParams,
};
use serde::Serialize;

use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::output::{new_table, write_table, Render};

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
        KvCmd::Set(c) => set(c, ctx).await,
        KvCmd::List(c) => list(c, ctx).await,
    }
}

async fn set(cmd: SetCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        SetCmd::Put { key, value } => {
            let value = if value == "-" { read_stdin()? } else { value };
            ctx.sdk
                .kvstore
                .create_set(&CreateSetParams {
                    key: key.clone(),
                    value,
                })
                .await?;
            ctx.out.note(&format!("✓ Set {key:?}"));
        }
        SetCmd::Get { key } => {
            let resp = ctx.sdk.kvstore.get_set(&key).await?;
            if ctx.out.json {
                crate::output::emit(&ctx.out, &resp)?;
            } else {
                println!("{}", resp.value);
            }
        }
        SetCmd::Ls(a) => {
            let resp = ctx
                .sdk
                .kvstore
                .get_sets(&GetSetsParams {
                    limit: a.limit,
                    cursor: a.cursor,
                })
                .await?;
            crate::output::emit(&ctx.out, &SetsView(resp))?;
        }
        SetCmd::Delete { key } => {
            confirm_mild(&ctx, &format!("Delete set {key:?}?"))?;
            ctx.sdk.kvstore.delete_set(&key).await?;
            ctx.out.note(&format!("✓ Deleted set {key:?}"));
        }
        SetCmd::Bulk(a) => {
            if a.add.is_empty() && a.delete.is_empty() {
                return Err(CliError::Arg(
                    "supply at least one --add or --delete".into(),
                ));
            }
            let mut add_sets = HashMap::new();
            for entry in a.add {
                let (k, v) = entry.split_once('=').ok_or_else(|| {
                    CliError::Arg(format!("--add expects KEY=VALUE, got {entry:?}"))
                })?;
                add_sets.insert(k.to_string(), v.to_string());
            }
            let params = BulkSetsParams {
                add_sets: if add_sets.is_empty() {
                    None
                } else {
                    Some(add_sets)
                },
                delete_sets: if a.delete.is_empty() {
                    None
                } else {
                    Some(a.delete)
                },
            };
            ctx.sdk.kvstore.bulk_sets(&params).await?;
            ctx.out.note("✓ Bulk sets applied");
        }
    }
    Ok(())
}

async fn list(cmd: ListCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        ListCmd::Ls(a) => {
            let resp = ctx
                .sdk
                .kvstore
                .get_lists(&GetListsParams {
                    limit: a.limit,
                    cursor: a.cursor,
                })
                .await?;
            crate::output::emit(&ctx.out, &ListsView(resp))?;
        }
        ListCmd::Get(a) => {
            let resp = ctx
                .sdk
                .kvstore
                .get_list(
                    &a.key,
                    &GetListParams {
                        limit: a.limit,
                        cursor: a.cursor,
                    },
                )
                .await?;
            crate::output::emit(&ctx.out, &ListView(resp))?;
        }
        ListCmd::Create { key, items } => {
            if items.is_empty() {
                return Err(CliError::Arg("supply at least one item".into()));
            }
            ctx.sdk
                .kvstore
                .create_list(&CreateListParams {
                    key: key.clone(),
                    items,
                })
                .await?;
            ctx.out.note(&format!("✓ Created list {key:?}"));
        }
        ListCmd::Append { key, item } => {
            ctx.sdk
                .kvstore
                .add_list_item(&key, &AddListItemParams { item: item.clone() })
                .await?;
            ctx.out.note(&format!("✓ Appended {item:?} to {key:?}"));
        }
        ListCmd::Contains { key, item } => {
            let resp = ctx.sdk.kvstore.list_contains_item(&key, &item).await?;
            if ctx.out.json {
                crate::output::emit(&ctx.out, &resp)?;
            } else {
                println!("{}", if resp.exists { "true" } else { "false" });
            }
        }
        ListCmd::RemoveItem { key, item } => {
            ctx.sdk.kvstore.delete_list_item(&key, &item).await?;
            ctx.out
                .note(&format!("✓ Removed {item:?} from list {key:?}"));
        }
        ListCmd::Update(a) => {
            if a.add_items.is_empty() && a.remove_items.is_empty() {
                return Err(CliError::Arg("supply --add and/or --remove".into()));
            }
            let params = UpdateListParams {
                add_items: if a.add_items.is_empty() {
                    None
                } else {
                    Some(a.add_items)
                },
                remove_items: if a.remove_items.is_empty() {
                    None
                } else {
                    Some(a.remove_items)
                },
            };
            ctx.sdk.kvstore.update_list(&a.key, &params).await?;
            ctx.out.note(&format!("✓ Updated list {:?}", a.key));
        }
        ListCmd::Delete { key } => {
            confirm_mild(&ctx, &format!("Delete list {key:?}?"))?;
            ctx.sdk.kvstore.delete_list(&key).await?;
            ctx.out.note(&format!("✓ Deleted list {key:?}"));
        }
    }
    Ok(())
}

fn confirm_mild(ctx: &Ctx, prompt: &str) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(prompt)?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    Ok(())
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

// ----- renderers ----- //

#[derive(Serialize)]
struct SetsView(quicknode_sdk::GetSetsResponse);

impl Render for SetsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table();
        t.set_header(vec!["KEY", "VALUE"]);
        for e in &self.0.data {
            t.add_row(vec![Cell::new(&e.key), Cell::new(&e.value)]);
        }
        write_table(w, &t)?;
        if !self.0.cursor.is_empty() {
            writeln!(w, "next: --cursor {}", self.0.cursor)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct ListsView(quicknode_sdk::GetListsResponse);

impl Render for ListsView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table();
        t.set_header(vec!["KEY"]);
        for k in &self.0.data.keys {
            t.add_row(vec![Cell::new(k)]);
        }
        write_table(w, &t)?;
        if !self.0.cursor.is_empty() {
            writeln!(w, "next: --cursor {}", self.0.cursor)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct ListView(quicknode_sdk::GetListResponse);

impl Render for ListView {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        for item in &self.0.data.items {
            writeln!(w, "{item}")?;
        }
        if !self.0.cursor.is_empty() {
            writeln!(w, "next: --cursor {}", self.0.cursor)?;
        }
        Ok(())
    }
}

impl Render for quicknode_sdk::GetSetResponse {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        writeln!(w, "{}", self.value)
    }
}

impl Render for quicknode_sdk::ListContainsItemResponse {
    fn render_table(
        &self,
        w: &mut dyn std::io::Write,
        _: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        writeln!(w, "{}", self.exists)
    }
}
