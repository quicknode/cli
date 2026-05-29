//! Top-level error type for the CLI and the SDK→user message mapping.

use std::path::PathBuf;

use quicknode_sdk::errors::{HttpKind, SdkError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("no API key found. Set QN_SDK__API_KEY or run 'qn auth login'")]
    NoApiKey,

    #[error("config file at {path} is invalid: {source}")]
    BadConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not write config file at {path}: {source}")]
    ConfigWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid argument: {0}")]
    Arg(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error(
        "operation requires confirmation; pass --yes to proceed without an interactive prompt"
    )]
    NeedsConfirmation,

    #[error(transparent)]
    Sdk(#[from] SdkError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Maps a [`CliError`] to a process exit code per the plan.
///
/// - 0: success (never produced here)
/// - 1: generic CLI failure (arg parse, IO, decode)
/// - 2: SdkError::Api (server returned a non-2xx)
/// - 3: SdkError::Http (network failure)
/// - 4: NoApiKey / BadConfig
/// - 5: user cancelled or needs --yes
pub fn exit_code_for(err: &CliError) -> i32 {
    match err {
        CliError::NoApiKey | CliError::BadConfig { .. } | CliError::ConfigWrite { .. } => 4,
        CliError::Cancelled | CliError::NeedsConfirmation => 5,
        CliError::Sdk(sdk) => match sdk {
            SdkError::Api { .. } => 2,
            SdkError::Http(_) => 3,
            _ => 1,
        },
        _ => 1,
    }
}

/// Renders the error to a human-friendly single line (or two for multi-line bodies).
///
/// Verbose mode appends the underlying body / source where available.
pub fn render(err: &CliError, verbose: bool) -> String {
    match err {
        CliError::Sdk(SdkError::Api { status, body }) => {
            let code = status.as_u16();
            let base = match code {
                401 | 403 => "unauthorized. Check your API key with 'qn auth whoami'.".to_string(),
                404 => "not found.".to_string(),
                422 => "the API rejected the request as invalid.".to_string(),
                429 => "rate limited by the QuickNode API. Try again shortly.".to_string(),
                500..=599 => format!(
                    "QuickNode API is having issues (HTTP {code}). Try again or check status.quicknode.com."
                ),
                _ => format!("API returned HTTP {code}."),
            };
            if verbose && !body.is_empty() {
                format!("Error: {base}\n{body}")
            } else {
                format!("Error: {base}")
            }
        }
        CliError::Sdk(sdk @ SdkError::Http(_)) => {
            let msg = match sdk.http_kind() {
                Some(HttpKind::Timeout) => {
                    "request timed out. Check your connection and try again."
                }
                Some(HttpKind::Connect) => {
                    "could not connect to api.quicknode.com. Check your network."
                }
                _ => "HTTP transport failure talking to the QuickNode API.",
            };
            if verbose {
                format!("Error: {msg}\n{sdk}")
            } else {
                format!("Error: {msg}")
            }
        }
        CliError::Sdk(SdkError::Decode { body, .. }) => {
            if verbose {
                format!("Error: unexpected response shape from API.\n{body}")
            } else {
                "Error: unexpected response shape from API. Re-run with --verbose to see the body."
                    .to_string()
            }
        }
        other => format!("Error: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quicknode_sdk::errors::SdkError;

    fn api_err(code: u16) -> CliError {
        CliError::Sdk(SdkError::Api {
            status: reqwest::StatusCode::from_u16(code).unwrap(),
            body: "{\"message\":\"boom\"}".to_string(),
        })
    }

    #[test]
    fn exit_code_api_is_2() {
        assert_eq!(exit_code_for(&api_err(404)), 2);
    }

    #[test]
    fn exit_code_no_api_key_is_4() {
        assert_eq!(exit_code_for(&CliError::NoApiKey), 4);
    }

    #[test]
    fn exit_code_cancelled_is_5() {
        assert_eq!(exit_code_for(&CliError::Cancelled), 5);
    }

    #[test]
    fn renders_401_as_unauthorized() {
        let msg = render(&api_err(401), false);
        assert!(msg.contains("unauthorized"), "got: {msg}");
    }

    #[test]
    fn renders_429_as_rate_limited() {
        let msg = render(&api_err(429), false);
        assert!(msg.contains("rate limited"), "got: {msg}");
    }

    #[test]
    fn renders_5xx_with_status() {
        let msg = render(&api_err(503), false);
        assert!(msg.contains("503"), "got: {msg}");
    }

    #[test]
    fn verbose_404_includes_body() {
        let msg = render(&api_err(404), true);
        assert!(msg.contains("boom"), "got: {msg}");
    }

    #[test]
    fn non_verbose_404_omits_body() {
        let msg = render(&api_err(404), false);
        assert!(!msg.contains("boom"), "got: {msg}");
    }
}
