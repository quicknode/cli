//! Destructive SDK calls with confirmation + success note.
//!
//! Builds on [`crate::confirm`] (which owns the *decision* — prompt, severity,
//! exit code) by adding the *execution* layer: prompt, then call the SDK, then
//! emit a uniform "✓ Deleted …" note. Commands hand in the SDK closure and the
//! resource label; the helper does the rest.
//!
//! Two entry points:
//! - [`single`]: one-resource delete (mild confirmation).
//! - [`all`]: bulk delete-all (severe `delete-all` confirmation).

use std::future::Future;

use quicknode_sdk::errors::SdkError;

use crate::confirm;
use crate::context::Ctx;
use crate::errors::CliError;

/// Single-resource destructive call. Mild confirmation; one `--yes` skips.
///
/// `resource` is the human label used in both prompt and note (e.g. `"webhook"`,
/// `"stream"`, `"tag"`, `"set"`, `"list"`). `id_display` is the pre-formatted
/// id — callers using URL-safe ids pass them raw; callers using free-form
/// strings (e.g. kv keys) pre-quote via `&format!("{key:?}")` so the prompt
/// and note disambiguate spaces / specials.
pub async fn single<F, Fut, T>(
    ctx: &Ctx,
    resource: &str,
    id_display: &str,
    call: F,
) -> Result<(), CliError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, SdkError>>,
{
    confirm::mild(ctx, &format!("Delete {resource} {id_display}?"))?;
    call().await?;
    ctx.out.note(&format!("✓ Deleted {resource} {id_display}"));
    Ok(())
}

/// Bulk delete-all destructive call. Severe confirmation; user must type
/// `delete-all` or pass `--yes --yes`.
///
/// `resource_singular` lands in the prompt ("…delete EVERY {singular}…"),
/// `resource_plural` lands in the success note ("✓ Deleted all {plural}").
pub async fn all<F, Fut, T>(
    ctx: &Ctx,
    resource_singular: &str,
    resource_plural: &str,
    call: F,
) -> Result<(), CliError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, SdkError>>,
{
    confirm::severe_delete_all(ctx, resource_singular)?;
    call().await?;
    ctx.out.note(&format!("✓ Deleted all {resource_plural}"));
    Ok(())
}
