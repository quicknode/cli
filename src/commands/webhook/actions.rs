//! Command bodies for `qn webhook …`.

use std::path::PathBuf;

use quicknode_sdk::webhooks::{
    ActivateWebhookParams, BitcoinWalletFilterByListTemplate, BitcoinWalletFilterInput,
    BitcoinWalletFilterTemplate, CreateWebhookFromTemplateParams, EvmAbiFilterByListTemplate,
    EvmAbiFilterInput, EvmAbiFilterTemplate, EvmContractEventsByListTemplate,
    EvmContractEventsInput, EvmContractEventsTemplate, EvmWalletFilterByListTemplate,
    EvmWalletFilterInput, EvmWalletFilterTemplate, GetWebhooksParams,
    HyperliquidWalletEventsFilterByListTemplate, HyperliquidWalletEventsFilterInput,
    HyperliquidWalletEventsFilterTemplate, SolanaWalletFilterByListTemplate,
    SolanaWalletFilterInput, SolanaWalletFilterTemplate,
    StellarWalletTransactionsFilterByListTemplate, StellarWalletTransactionsFilterInput,
    StellarWalletTransactionsFilterTemplate, TemplateArgs, UpdateWebhookParams,
    UpdateWebhookTemplateParams, WebhookDestinationAttributes, XrplWalletFilterByListTemplate,
    XrplWalletFilterInput, XrplWalletFilterTemplate,
};

use super::render::{WebhookView, WebhooksListView};
use super::{ActivateArgs, CreateArgs, ListArgs, TemplateKind, UpdateArgs, UpdateTemplateArgs};
use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::retry::retrying;

pub(super) async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = GetWebhooksParams {
        limit: a.limit,
        offset: a.offset,
    };
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.webhooks.list_webhooks(&params)
    })
    .await?;
    crate::output::emit(&ctx.out, &WebhooksListView(resp))
}

pub(super) async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let w = retrying(ctx.global.retries, || ctx.sdk.webhooks.get_webhook(id)).await?;
    crate::output::emit(&ctx.out, &WebhookView(w))
}

pub(super) async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let template_args = build_template_args(TemplateInputs {
        kind: a.template,
        wallets: a.wallets,
        accounts: a.accounts,
        contracts: a.contracts,
        event_hashes: a.event_hashes,
        abi: a.abi,
        abi_file: a.abi_file,
        wallets_list_name: a.wallets_list_name,
        accounts_list_name: a.accounts_list_name,
        contracts_list_name: a.contracts_list_name,
        event_hashes_list_name: a.event_hashes_list_name,
    })?;
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
        Some(url) => Some(build_destination(url, a.security_token, a.compression)?),
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
    let template_args = build_template_args(TemplateInputs {
        kind: a.template,
        wallets: a.wallets,
        accounts: a.accounts,
        contracts: a.contracts,
        event_hashes: a.event_hashes,
        abi: a.abi,
        abi_file: a.abi_file,
        wallets_list_name: a.wallets_list_name,
        accounts_list_name: a.accounts_list_name,
        contracts_list_name: a.contracts_list_name,
        event_hashes_list_name: a.event_hashes_list_name,
    })?;
    let destination = match a.url {
        Some(url) => Some(build_destination(url, a.security_token, a.compression)?),
        None => None,
    };
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
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete webhook {id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.webhooks.delete_webhook(id).await?;
    ctx.out.note(&format!("✓ Deleted webhook {id}"));
    Ok(())
}

// There is intentionally no `webhook delete-all` command. Account-wide wipes
// are out of scope for the CLI; use the API directly if you really need one.

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
    let resp = retrying(ctx.global.retries, || ctx.sdk.webhooks.get_enabled_count()).await?;
    if ctx.out.format.is_structured() {
        crate::output::emit(&ctx.out, &resp)
    } else {
        println!("{}", resp.total);
        Ok(())
    }
}

/// All template-selection inputs gathered from the CLI flags. Each template
/// reads the subset it needs; the rest stay unused for that variant.
struct TemplateInputs {
    kind: TemplateKind,
    wallets: Vec<String>,
    accounts: Vec<String>,
    contracts: Vec<String>,
    event_hashes: Vec<String>,
    abi: Option<String>,
    abi_file: Option<PathBuf>,
    wallets_list_name: Option<String>,
    accounts_list_name: Option<String>,
    contracts_list_name: Option<String>,
    event_hashes_list_name: Option<String>,
}

/// Resolved value source for a single template field: either inline values
/// supplied directly, or the name of a saved list to reference.
enum FilterSource {
    Inline(Vec<String>),
    ByList(String),
}

fn build_template_args(t: TemplateInputs) -> Result<TemplateArgs, CliError> {
    match t.kind {
        TemplateKind::EvmWallet => Ok(match wallets_source(t.wallets, t.wallets_list_name)? {
            FilterSource::Inline(wallets) => TemplateArgs::EvmWalletFilter(
                EvmWalletFilterInput::Inline(EvmWalletFilterTemplate { wallets }),
            ),
            FilterSource::ByList(wallets_list_name) => TemplateArgs::EvmWalletFilter(
                EvmWalletFilterInput::ByList(EvmWalletFilterByListTemplate { wallets_list_name }),
            ),
        }),
        TemplateKind::BitcoinWallet => Ok(match wallets_source(t.wallets, t.wallets_list_name)? {
            FilterSource::Inline(wallets) => TemplateArgs::BitcoinWalletFilter(
                BitcoinWalletFilterInput::Inline(BitcoinWalletFilterTemplate { wallets }),
            ),
            FilterSource::ByList(wallets_list_name) => {
                TemplateArgs::BitcoinWalletFilter(BitcoinWalletFilterInput::ByList(
                    BitcoinWalletFilterByListTemplate { wallets_list_name },
                ))
            }
        }),
        TemplateKind::XrplWallet => Ok(match wallets_source(t.wallets, t.wallets_list_name)? {
            FilterSource::Inline(wallets) => TemplateArgs::XrplWalletFilter(
                XrplWalletFilterInput::Inline(XrplWalletFilterTemplate { wallets }),
            ),
            FilterSource::ByList(wallets_list_name) => TemplateArgs::XrplWalletFilter(
                XrplWalletFilterInput::ByList(XrplWalletFilterByListTemplate { wallets_list_name }),
            ),
        }),
        TemplateKind::HyperliquidWalletEvents => {
            Ok(match wallets_source(t.wallets, t.wallets_list_name)? {
                FilterSource::Inline(wallets) => TemplateArgs::HyperliquidWalletEventsFilter(
                    HyperliquidWalletEventsFilterInput::Inline(
                        HyperliquidWalletEventsFilterTemplate { wallets },
                    ),
                ),
                FilterSource::ByList(wallets_list_name) => {
                    TemplateArgs::HyperliquidWalletEventsFilter(
                        HyperliquidWalletEventsFilterInput::ByList(
                            HyperliquidWalletEventsFilterByListTemplate { wallets_list_name },
                        ),
                    )
                }
            })
        }
        TemplateKind::StellarWalletTransactions => {
            Ok(match wallets_source(t.wallets, t.wallets_list_name)? {
                FilterSource::Inline(wallets) => {
                    TemplateArgs::StellarWalletTransactionsSourceAccountFilter(
                        StellarWalletTransactionsFilterInput::Inline(
                            StellarWalletTransactionsFilterTemplate { wallets },
                        ),
                    )
                }
                FilterSource::ByList(wallets_list_name) => {
                    TemplateArgs::StellarWalletTransactionsSourceAccountFilter(
                        StellarWalletTransactionsFilterInput::ByList(
                            StellarWalletTransactionsFilterByListTemplate { wallets_list_name },
                        ),
                    )
                }
            })
        }
        TemplateKind::SolanaWallet => {
            let source = filter_source(
                t.accounts,
                t.accounts_list_name,
                "--account",
                "--accounts-list-name",
            )?;
            Ok(match source {
                FilterSource::Inline(accounts) => TemplateArgs::SolanaWalletFilter(
                    SolanaWalletFilterInput::Inline(SolanaWalletFilterTemplate { accounts }),
                ),
                FilterSource::ByList(accounts_list_name) => {
                    TemplateArgs::SolanaWalletFilter(SolanaWalletFilterInput::ByList(
                        SolanaWalletFilterByListTemplate { accounts_list_name },
                    ))
                }
            })
        }
        TemplateKind::EvmContractEvents => {
            let source = filter_source(
                t.contracts,
                t.contracts_list_name,
                "--contract",
                "--contracts-list-name",
            )?;
            Ok(match source {
                FilterSource::Inline(contracts) => TemplateArgs::EvmContractEvents(
                    EvmContractEventsInput::Inline(EvmContractEventsTemplate {
                        contracts,
                        event_hashes: t.event_hashes,
                    }),
                ),
                FilterSource::ByList(contracts_list_name) => TemplateArgs::EvmContractEvents(
                    EvmContractEventsInput::ByList(EvmContractEventsByListTemplate {
                        contracts_list_name,
                        event_hashes_list_name: t.event_hashes_list_name,
                    }),
                ),
            })
        }
        TemplateKind::EvmAbi => {
            let abi_text = read_abi(t.abi, t.abi_file)?;
            // The ABI is always inline content; only the contracts can come
            // from a saved list, which selects the ByList variant.
            Ok(match t.contracts_list_name {
                Some(contracts_list_name) => {
                    if !t.contracts.is_empty() {
                        return Err(CliError::Arg(
                            "supply either --contract or --contracts-list-name, not both".into(),
                        ));
                    }
                    TemplateArgs::EvmAbiFilter(EvmAbiFilterInput::ByList(
                        EvmAbiFilterByListTemplate {
                            abi_json: abi_text,
                            contracts_list_name: Some(contracts_list_name),
                        },
                    ))
                }
                None => {
                    if t.contracts.is_empty() {
                        return Err(CliError::Arg(
                            "supply at least one --contract (or --contracts-list-name)".into(),
                        ));
                    }
                    TemplateArgs::EvmAbiFilter(EvmAbiFilterInput::Inline(EvmAbiFilterTemplate {
                        abi: abi_text,
                        contracts: t.contracts,
                    }))
                }
            })
        }
    }
}

/// Wallet-style templates all key off the same `--wallet` / `--wallets-list-name` pair.
fn wallets_source(
    wallets: Vec<String>,
    wallets_list_name: Option<String>,
) -> Result<FilterSource, CliError> {
    filter_source(
        wallets,
        wallets_list_name,
        "--wallet",
        "--wallets-list-name",
    )
}

/// Resolve an inline-values-vs-saved-list choice, rejecting "both" and "neither".
fn filter_source(
    inline: Vec<String>,
    list_name: Option<String>,
    inline_flag: &str,
    list_flag: &str,
) -> Result<FilterSource, CliError> {
    match list_name {
        Some(name) => {
            if !inline.is_empty() {
                return Err(CliError::Arg(format!(
                    "supply either {inline_flag} or {list_flag}, not both"
                )));
            }
            Ok(FilterSource::ByList(name))
        }
        None => {
            if inline.is_empty() {
                return Err(CliError::Arg(format!(
                    "supply at least one {inline_flag} (or {list_flag})"
                )));
            }
            Ok(FilterSource::Inline(inline))
        }
    }
}

fn read_abi(abi: Option<String>, abi_file: Option<PathBuf>) -> Result<String, CliError> {
    match (abi, abi_file) {
        (Some(s), None) => Ok(s),
        (None, Some(p)) => Ok(std::fs::read_to_string(&p)?),
        (None, None) => Err(CliError::Arg("supply --abi or --abi-file".into())),
        (Some(_), Some(_)) => Err(CliError::Arg(
            "supply only one of --abi or --abi-file".into(),
        )),
    }
}

/// Build a destination from an explicit URL, requiring compression (the API
/// needs a non-optional value whenever a destination is sent).
fn build_destination(
    url: String,
    security_token: Option<String>,
    compression: Option<String>,
) -> Result<WebhookDestinationAttributes, CliError> {
    let compression = compression
        .ok_or_else(|| CliError::Arg("--url requires --compression (`gzip` or `none`)".into()))?;
    Ok(WebhookDestinationAttributes {
        url,
        security_token,
        compression,
    })
}
