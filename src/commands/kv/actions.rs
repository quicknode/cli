//! Command bodies for `qn kv …`.

use std::collections::HashMap;

use quicknode_sdk::{
    AddListItemParams, BulkSetsParams, CreateListParams, CreateSetParams, GetListParams,
    GetListsParams, GetSetsParams, UpdateListParams,
};

use super::render::{ListView, ListsView, SetsView};
use super::{ListCmd, SetCmd};
use crate::context::Ctx;
use crate::errors::CliError;

pub(super) async fn set(cmd: SetCmd, ctx: Ctx) -> Result<(), CliError> {
    match cmd {
        SetCmd::Put { key, value } => {
            let value = if value == "-" {
                super::read_stdin()?
            } else {
                value
            };
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
            if ctx.out.format.is_structured() {
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
            super::confirm_mild(&ctx, &format!("Delete set {key:?}?"))?;
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

pub(super) async fn list(cmd: ListCmd, ctx: Ctx) -> Result<(), CliError> {
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
            if ctx.out.format.is_structured() {
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
            super::confirm_mild(&ctx, &format!("Delete list {key:?}?"))?;
            ctx.sdk.kvstore.delete_list(&key).await?;
            ctx.out.note(&format!("✓ Deleted list {key:?}"));
        }
    }
    Ok(())
}
