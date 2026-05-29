//! Destructive-action confirmation helpers.
//!
//! Two levels:
//! - **mild** (e.g. `endpoint archive`, `tag delete`): one y/N prompt; --yes skips.
//! - **severe** (e.g. `stream delete-all`): typed-word confirmation; --yes --yes (twice) skips.
//!
//! On a non-TTY, mild requires `--yes` and severe requires `--yes --yes` — never
//! auto-confirm in scripts.

use crate::errors::CliError;

/// What kind of confirmation a command needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Mild,
    Severe,
}

/// Configuration captured from CLI flags + TTY detection.
#[derive(Debug, Clone, Copy)]
pub struct ConfirmCfg {
    pub yes_count: u8,
    pub no_input: bool,
    pub is_tty: bool,
}

impl ConfirmCfg {
    pub fn new(yes_count: u8, no_input: bool, is_tty: bool) -> Self {
        Self {
            yes_count,
            no_input,
            is_tty,
        }
    }
}

/// Decide whether to proceed *without* prompting.
///
/// Returns:
/// - `Ok(true)` → proceed, no prompt needed
/// - `Ok(false)` → caller should prompt the user (only possible on TTY w/o --no-input)
/// - `Err(NeedsConfirmation)` → cannot proceed (script mode without enough --yes)
pub fn decide_without_prompt(severity: Severity, cfg: ConfirmCfg) -> Result<bool, CliError> {
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
pub fn prompt_yes_no(message: &str) -> Result<bool, CliError> {
    use dialoguer::Confirm;
    Confirm::new()
        .with_prompt(message)
        .default(false)
        .interact()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))
}

/// Interactive typed-word confirmation. Returns true if the user types
/// `expected` exactly.
pub fn prompt_typed(message: &str, expected: &str) -> Result<bool, CliError> {
    use dialoguer::Input;
    let typed: String = Input::new()
        .with_prompt(message)
        .allow_empty(true)
        .interact_text()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?;
    Ok(typed == expected)
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
