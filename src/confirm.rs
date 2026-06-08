//! Destructive-action confirmation helpers.
//!
//! Two levels:
//! - **mild** (e.g. `endpoint archive`, `tag delete`): one y/N prompt; --yes skips.
//! - **severe** (e.g. `stream delete-all`): typed-word confirmation; --yes --yes (twice) skips.
//!
//! On a non-TTY, mild requires `--yes` and severe requires `--yes --yes` — never
//! auto-confirm in scripts.
//!
//! Commands call [`mild`] or [`severe_delete_all`]; the lower-level primitives
//! are `pub(crate)` so call sites can't drift back into copy-pasting the
//! decide → prompt → map_err ceremony.

use crate::context::Ctx;
use crate::errors::CliError;

/// What kind of confirmation a command needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Mild,
    Severe,
}

/// Configuration captured from CLI flags + TTY detection.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfirmCfg {
    pub yes_count: u8,
    pub no_input: bool,
    pub is_tty: bool,
}

impl ConfirmCfg {
    pub(crate) fn new(yes_count: u8, no_input: bool, is_tty: bool) -> Self {
        Self {
            yes_count,
            no_input,
            is_tty,
        }
    }

    fn from_ctx(ctx: &Ctx) -> Self {
        Self::new(
            ctx.global.yes_count,
            ctx.global.no_input,
            ctx.out.stdout_is_tty,
        )
    }
}

/// Decide whether to proceed *without* prompting.
///
/// Returns:
/// - `Ok(true)` → proceed, no prompt needed
/// - `Ok(false)` → caller should prompt the user (only possible on TTY w/o --no-input)
/// - `Err(NeedsConfirmation)` → cannot proceed (script mode without enough --yes)
pub(crate) fn decide_without_prompt(severity: Severity, cfg: ConfirmCfg) -> Result<bool, CliError> {
    let required = match severity {
        Severity::Mild => 1,
        Severity::Severe => 2,
    };
    if cfg.yes_count >= required {
        return Ok(true);
    }
    if !cfg.is_tty || cfg.no_input {
        return Err(CliError::NeedsConfirmation);
    }
    Ok(false)
}

/// Interactive y/N prompt. Returns true on yes.
pub(crate) fn prompt_yes_no(message: &str) -> Result<bool, CliError> {
    use dialoguer::Confirm;
    Confirm::new()
        .with_prompt(message)
        .default(false)
        .interact()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))
}

/// Interactive typed-word confirmation. Returns true if the user types
/// `expected` exactly.
pub(crate) fn prompt_typed(message: &str, expected: &str) -> Result<bool, CliError> {
    use dialoguer::Input;
    let typed: String = Input::new()
        .with_prompt(message)
        .allow_empty(true)
        .interact_text()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?;
    Ok(typed == expected)
}

/// Mild confirmation gate: returns `Ok(())` if the caller may proceed, or
/// `Err(Cancelled)` / `Err(NeedsConfirmation)` if not. One `--yes` skips the
/// prompt; on a TTY without `--yes` the user gets a y/N; on a non-TTY without
/// `--yes` the call returns `NeedsConfirmation` (exit code 5).
pub fn mild(ctx: &Ctx, prompt: &str) -> Result<(), CliError> {
    let cfg = ConfirmCfg::from_ctx(ctx);
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(prompt)?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    Ok(())
}

/// Severe confirmation gate for the `delete-all` family. Requires the user
/// to type the literal word `delete-all`, or `--yes --yes` to skip. The
/// prompt is uniform: `"Type 'delete-all' to delete EVERY {singular_resource}
/// on the account"`.
pub fn severe_delete_all(ctx: &Ctx, singular_resource: &str) -> Result<(), CliError> {
    let cfg = ConfirmCfg::from_ctx(ctx);
    let proceed = match decide_without_prompt(Severity::Severe, cfg)? {
        true => true,
        false => prompt_typed(
            &format!("Type 'delete-all' to delete EVERY {singular_resource} on the account"),
            "delete-all",
        )?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mild_with_one_yes_is_auto() {
        let cfg = ConfirmCfg::new(1, false, true);
        assert!(decide_without_prompt(Severity::Mild, cfg).unwrap());
    }

    #[test]
    fn mild_with_two_yes_is_still_auto() {
        let cfg = ConfirmCfg::new(2, false, false);
        assert!(decide_without_prompt(Severity::Mild, cfg).unwrap());
    }

    #[test]
    fn mild_tty_no_yes_returns_false_so_caller_prompts() {
        let cfg = ConfirmCfg::new(0, false, true);
        assert!(!decide_without_prompt(Severity::Mild, cfg).unwrap());
    }

    #[test]
    fn mild_non_tty_no_yes_needs_confirmation() {
        let cfg = ConfirmCfg::new(0, false, false);
        let err = decide_without_prompt(Severity::Mild, cfg).unwrap_err();
        assert!(matches!(err, CliError::NeedsConfirmation));
    }

    #[test]
    fn mild_no_input_no_yes_needs_confirmation_even_on_tty() {
        let cfg = ConfirmCfg::new(0, true, true);
        let err = decide_without_prompt(Severity::Mild, cfg).unwrap_err();
        assert!(matches!(err, CliError::NeedsConfirmation));
    }

    #[test]
    fn severe_needs_two_yes() {
        let cfg = ConfirmCfg::new(1, false, false);
        let err = decide_without_prompt(Severity::Severe, cfg).unwrap_err();
        assert!(matches!(err, CliError::NeedsConfirmation));
    }

    #[test]
    fn severe_with_two_yes_proceeds() {
        let cfg = ConfirmCfg::new(2, false, false);
        assert!(decide_without_prompt(Severity::Severe, cfg).unwrap());
    }

    #[test]
    fn severe_tty_prompts() {
        let cfg = ConfirmCfg::new(0, false, true);
        assert!(!decide_without_prompt(Severity::Severe, cfg).unwrap());
    }
}
