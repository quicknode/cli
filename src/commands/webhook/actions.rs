//! Command bodies for `qn webhook …`.

use std::path::PathBuf;

use quicknode_sdk::webhooks::{
    ActivateWebhookParams, BitcoinWalletFilterTemplate, CreateWebhookFromTemplateParams,
    EvmAbiFilterTemplate, EvmContractEventsTemplate, EvmWalletFilterTemplate, GetWebhooksParams,
    HyperliquidWalletEventsFilterTemplate, SolanaWalletFilterTemplate,
    StellarWalletTransactionsFilterTemplate, TemplateArgs, UpdateWebhookParams,
    UpdateWebhookTemplateParams, WebhookDestinationAttributes, XrplWalletFilterTemplate,
};

use super::render::{WebhookView, WebhooksListView};
use super::{ActivateArgs, CreateArgs, ListArgs, TemplateKind, UpdateArgs, UpdateTemplateArgs};
use crate::context::Ctx;
use crate::destroy;
use crate::errors::CliError;

pub(super) async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = GetWebhooksParams {
        limit: a.limit,
        offset: a.offset,
    };
    let resp = ctx.sdk.webhooks.list_webhooks(&params).await?;
    crate::output::emit(&ctx.out, &WebhooksListView(resp))
}

pub(super) async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let w = ctx.sdk.webhooks.get_webhook(id).await?;
    crate::output::emit(&ctx.out, &WebhookView(w))
}

pub(super) async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let template_args = build_template_args(
        a.template,
        a.wallets,
        a.accounts,
        a.contracts,
        a.event_hashes,
        a.abi,
        a.abi_file,
    )?;
    let params = CreateWebhookFromTemplateParams {
        name: a.name,
        network: a.network,
        notification_email: a.notification_email,
        destination_attributes: WebhookDestinationAttributes {
            url: a.url,
            security_token: a.security_token,
            compression: a.compression,
        },
        template_args,
    };
    let w = ctx
        .sdk
        .webhooks
        .create_webhook_from_template(&params)
        .await?;
    ctx.out.note(&format!("✓ Created webhook {}", w.id));
    crate::output::emit(&ctx.out, &WebhookView(w))
}

pub(super) async fn update(a: UpdateArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.name.is_none()
        && a.notification_email.is_none()
        && a.url.is_none()
        && a.security_token.is_none()
        && a.compression.is_none()
    {
        return Err(CliError::Arg(
            "supply at least one of --name, --notification-email, --url, --security-token, --compression".into(),
        ));
    }
    let destination = match a.url {
        Some(url) => Some(WebhookDestinationAttributes {
            url,
            security_token: a.security_token,
            compression: a.compression,
        }),
        None if a.security_token.is_some() || a.compression.is_some() => {
            return Err(CliError::Arg(
                "to change destination fields, also supply --url".into(),
            ));
        }
        None => None,
    };
    let params = UpdateWebhookParams {
        name: a.name,
        notification_email: a.notification_email,
        destination_attributes: destination,
    };
    let w = ctx.sdk.webhooks.update_webhook(&a.id, &params).await?;
    ctx.out.note(&format!("✓ Updated webhook {}", a.id));
    crate::output::emit(&ctx.out, &WebhookView(w))
}

pub(super) async fn update_template(a: UpdateTemplateArgs, ctx: Ctx) -> Result<(), CliError> {
    let template_args = build_template_args(
        a.template,
        a.wallets,
        a.accounts,
        a.contracts,
        a.event_hashes,
        a.abi,
        a.abi_file,
    )?;
    let destination = a.url.map(|url| WebhookDestinationAttributes {
        url,
        security_token: a.security_token,
        compression: a.compression,
    });
    let params = UpdateWebhookTemplateParams {
        name: a.name,
        notification_email: a.notification_email,
        destination_attributes: destination,
        template_args,
    };
    let w = ctx
        .sdk
        .webhooks
        .update_webhook_template(&a.id, &params)
        .await?;
    ctx.out
        .note(&format!("✓ Updated template on webhook {}", a.id));
    crate::output::emit(&ctx.out, &WebhookView(w))
}

pub(super) async fn delete(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let webhooks = &ctx.sdk.webhooks;
    destroy::single(&ctx, "webhook", id, || webhooks.delete_webhook(id)).await
}

pub(super) async fn delete_all(ctx: Ctx) -> Result<(), CliError> {
    let webhooks = &ctx.sdk.webhooks;
    destroy::all(&ctx, "webhook", "webhooks", || {
        webhooks.delete_all_webhooks()
    })
    .await
}

pub(super) async fn activate(a: ActivateArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = ActivateWebhookParams {
        start_from: a.start_from.into(),
    };
    ctx.sdk.webhooks.activate_webhook(&a.id, &params).await?;
    ctx.out.note(&format!("✓ Activated webhook {}", a.id));
    Ok(())
}

pub(super) async fn pause(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.webhooks.pause_webhook(id).await?;
    ctx.out.note(&format!("✓ Paused webhook {id}"));
    Ok(())
}

pub(super) async fn enabled_count(ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx.sdk.webhooks.get_enabled_count().await?;
    if ctx.out.format.is_structured() {
        crate::output::emit(&ctx.out, &resp)
    } else {
        println!("{}", resp.total);
        Ok(())
    }
}

fn build_template_args(
    kind: TemplateKind,
    wallets: Vec<String>,
    accounts: Vec<String>,
    contracts: Vec<String>,
    event_hashes: Vec<String>,
    abi: Option<String>,
    abi_file: Option<PathBuf>,
) -> Result<TemplateArgs, CliError> {
    let event_hashes_opt = if event_hashes.is_empty() {
        None
    } else {
        Some(event_hashes)
    };
    match kind {
        TemplateKind::EvmWallet => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::EvmWalletFilter(EvmWalletFilterTemplate {
                wallets,
            }))
        }
        TemplateKind::SolanaWallet => {
            if accounts.is_empty() {
                return Err(CliError::Arg("supply at least one --account".into()));
            }
            Ok(TemplateArgs::SolanaWalletFilter(
                SolanaWalletFilterTemplate { accounts },
            ))
        }
        TemplateKind::BitcoinWallet => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::BitcoinWalletFilter(
                BitcoinWalletFilterTemplate { wallets },
            ))
        }
        TemplateKind::XrplWallet => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::XrplWalletFilter(XrplWalletFilterTemplate {
                wallets,
            }))
        }
        TemplateKind::HyperliquidWalletEvents => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::HyperliquidWalletEventsFilter(
                HyperliquidWalletEventsFilterTemplate { wallets },
            ))
        }
        TemplateKind::StellarWalletTransactions => {
            require_wallets(&wallets)?;
            Ok(TemplateArgs::StellarWalletTransactionsSourceAccountFilter(
                StellarWalletTransactionsFilterTemplate { wallets },
            ))
        }
        TemplateKind::EvmContractEvents => {
            if contracts.is_empty() {
                return Err(CliError::Arg("supply at least one --contract".into()));
            }
            Ok(TemplateArgs::EvmContractEvents(EvmContractEventsTemplate {
                contracts,
                event_hashes: event_hashes_opt,
            }))
        }
        TemplateKind::EvmAbi => {
            if contracts.is_empty() {
                return Err(CliError::Arg("supply at least one --contract".into()));
            }
            let abi_text = match (abi, abi_file) {
                (Some(s), None) => s,
                (None, Some(p)) => std::fs::read_to_string(&p)?,
                (None, None) => {
                    return Err(CliError::Arg("supply --abi or --abi-file".into()));
                }
                (Some(_), Some(_)) => {
                    return Err(CliError::Arg(
                        "supply only one of --abi or --abi-file".into(),
                    ));
                }
            };
            Ok(TemplateArgs::EvmAbiFilter(EvmAbiFilterTemplate {
                abi: abi_text,
                contracts,
            }))
        }
    }
}

fn require_wallets(wallets: &[String]) -> Result<(), CliError> {
    if wallets.is_empty() {
        Err(CliError::Arg("supply at least one --wallet".into()))
    } else {
        Ok(())
    }
}
