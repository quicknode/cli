//! Top-level error type for the CLI and the SDK→user message mapping.

use std::collections::BTreeSet;
use std::path::PathBuf;

use quicknode_sdk::errors::{HttpKind, SdkError};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("no API key found. Run 'qn auth login', or pass --api-key or --config-file")]
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

    /// A paid RPC call failed in a way where the payment may already have
    /// settled (e.g. the gateway's post-payment response could not be
    /// interpreted). Kept separate from `Sdk` so it maps to exit 3 and renders
    /// the check-your-wallet guidance; the paid lane must never auto-retry it.
    #[error(
        "the paid request's outcome is unknown — the payment may have been settled; \
         check your wallet before retrying"
    )]
    PaymentMaybeCharged(#[source] SdkError),

    /// The gateway refused a paid request and nothing settled (out of credits,
    /// monthly limit, an exhausted channel). Carries an actionable message and
    /// maps to exit 2 — the "refused, nothing settled" bucket — so scripts can
    /// distinguish it from a generic arg error (exit 1) or an unknown outcome
    /// (exit 3).
    #[error("{0}")]
    PaymentRefused(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("could not serialize output: {0}")]
    Format(String),
}

/// Maps a [`CliError`] to a process exit code per the plan.
///
/// - 0: success (never produced here)
/// - 1: generic CLI failure (arg parse, IO, decode). clap usage errors are
///   mapped to 1 in main.rs too, so 2 always and only means an API error.
/// - 2: SdkError::Api (server returned a non-2xx); also a payment the gateway
///   refused without settling (PaymentRejected 4xx / PaymentUnsupported —
///   the paid lane wraps 5xx rejections into PaymentMaybeCharged first)
/// - 3: SdkError::Http (network failure); also an unknown payment outcome
///   (PaymentIndeterminate / PaymentMaybeCharged — the payment was
///   submitted, the caller may have been charged)
/// - 4: NoApiKey / BadConfig
/// - 5: user cancelled or needs --yes
pub fn exit_code_for(err: &CliError) -> i32 {
    match err {
        CliError::NoApiKey | CliError::BadConfig { .. } | CliError::ConfigWrite { .. } => 4,
        CliError::Cancelled | CliError::NeedsConfirmation => 5,
        CliError::PaymentMaybeCharged(_) => 3,
        CliError::PaymentRefused(_) => 2,
        CliError::Sdk(sdk) => match sdk {
            SdkError::Api { .. } => 2,
            SdkError::Http(_) => 3,
            SdkError::PaymentUnsupported { .. } | SdkError::PaymentRejected { .. } => 2,
            SdkError::PaymentIndeterminate => 3,
            _ => 1,
        },
        _ => 1,
    }
}

/// Renders the error to a human-friendly message using the real process argv
/// for did-you-mean suggestions. Use [`render_with_argv`] from tests where the
/// simulated argv differs from the process argv.
///
/// Verbose mode appends the underlying body / source where available.
pub fn render(err: &CliError, verbose: bool) -> String {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    render_with_argv(err, verbose, &argv)
}

/// Like [`render`] but uses the supplied argv values for did-you-mean lookup.
pub fn render_with_argv(err: &CliError, verbose: bool, argv: &[String]) -> String {
    match err {
        CliError::Sdk(SdkError::PaymentUnsupported { offered }) => {
            format!(
                "Error: no offered payment option matched your configuration \
                 (check --payment-network, --payment-asset, and --max-amount). Nothing was charged.\n\
                 Gateway offered: {offered}"
            )
        }
        CliError::Sdk(SdkError::PaymentRejected { status, body }) => {
            // Only 4xx rejections reach this arm from the paid lane (5xx are
            // wrapped into PaymentMaybeCharged): the gateway refused the
            // credential without settling it. The SDK has already reduced the
            // body to the gateway's own reason when it was the JSON error shape.
            let reason = payment_rejection_reason(body);
            let mut msg = format!("Error: the gateway refused the payment (HTTP {status}).");
            if let Some(r) = &reason {
                let r = r.trim_end_matches('.');
                msg.push_str(&format!(" Gateway: {r}."));
            }
            msg.push_str(
                " The signed payment was not accepted, so nothing should have settled. \
                 Common causes: the wallet is unfunded, or --payment-network/--payment-asset/--max-amount \
                 don't match an offer (see 'qn rpc pay-networks').",
            );
            // When the reason wasn't a clean one-liner, append the raw body under
            // --verbose for the full detail.
            if verbose && reason.is_none() && !body.is_empty() {
                format!("{msg}\n{body}")
            } else {
                msg
            }
        }
        CliError::Sdk(SdkError::PaymentIndeterminate) => {
            "Error: the paid request was sent but its response was lost — the request \
             may have been settled; check your wallet before retrying. Do not blindly \
             re-run this command."
                .to_string()
        }
        CliError::PaymentMaybeCharged(source) => {
            let msg = "Error: the paid request failed after the payment was submitted — \
                       the payment may have been settled; check your wallet before \
                       retrying. Do not blindly re-run this command.";
            if verbose {
                format!("{msg}\n{source}")
            } else {
                format!("{msg} Re-run with --verbose for the response detail.")
            }
        }
        CliError::Sdk(SdkError::Api { status, body }) => {
            render_api_error(status.as_u16(), body, verbose, argv)
        }
        CliError::Sdk(sdk @ SdkError::Http(inner)) => {
            // The failed host varies: control-plane calls hit api.quicknode.com,
            // RPC data-plane calls hit the endpoint host (*.quiknode.pro). Name
            // the actual host from the reqwest error's URL when available rather
            // than hardcoding one.
            let host = inner.url().and_then(|u| u.host_str()).map(str::to_string);
            let target = host
                .map(|h| format!("'{h}'"))
                .unwrap_or_else(|| "the Quicknode API".to_string());
            let msg = match sdk.http_kind() {
                Some(HttpKind::Timeout) => {
                    format!("request to {target} timed out. Check your connection and try again.")
                }
                Some(HttpKind::Connect) => {
                    format!("could not connect to {target}. Check your network.")
                }
                _ => format!("HTTP transport failure talking to {target}."),
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
        CliError::BadConfig { path, source } => {
            if verbose {
                format!(
                    "Error: config file at {} is invalid: {source}",
                    path.display()
                )
            } else {
                format!(
                    "Error: config file at {} is invalid. Re-run with --verbose for details.",
                    path.display()
                )
            }
        }
        other => format!("Error: {other}"),
    }
}

/// Status codes have a small set of canonical user-facing messages. Validation
/// (400/422) gets the structured body treatment from `parse_api_body`.
/// The gateway's own rejection reason, when the body is a clean short string
/// (the SDK reduces its JSON `{error, message}` shape to that before this
/// point). A long body — e.g. a full 402 payment menu — is not a reason, so it
/// returns `None` and the caller falls back to the generic guidance (plus the
/// raw body under `--verbose`).
fn payment_rejection_reason(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.len() > 200 || trimmed.starts_with('{') {
        return None;
    }
    Some(trimmed.to_string())
}

fn render_api_error(code: u16, body: &str, verbose: bool, argv: &[String]) -> String {
    let headline = match code {
        400 | 422 => "invalid request.".to_string(),
        401 | 403 => "unauthorized. Check your API key with 'qn auth whoami'.".to_string(),
        404 => "not found.".to_string(),
        429 => "rate limited by the Quicknode API. Try again shortly.".to_string(),
        500..=599 => format!(
            "something went wrong (HTTP {code}). Please try again; if the problem persists, \
             contact support at https://support.quicknode.com."
        ),
        _ => format!("API returned HTTP {code}."),
    };

    // For non-validation status codes, body is mostly noise (server stack traces,
    // HTML error pages, etc). Only mine it for validation-class errors.
    let parsed = if matches!(code, 400 | 422) {
        parse_api_body(body, argv)
    } else {
        ParsedApiBody::default()
    };

    let mut out = format!("Error: {headline}");

    if !parsed.bullets.is_empty() {
        for bullet in &parsed.bullets {
            out.push_str("\n  • ");
            out.push_str(bullet);
        }
    } else if matches!(code, 400 | 422) && !body.is_empty() && !verbose {
        // We tried to parse and got nothing useful; surface the raw body so
        // the user isn't left with a bare "invalid request." line.
        out.push('\n');
        out.push_str(body.trim());
    }

    for hint in &parsed.hints {
        out.push('\n');
        out.push_str(hint);
    }

    if verbose && !body.is_empty() {
        out.push('\n');
        out.push_str(body);
    } else if matches!(code, 400 | 422) && !parsed.bullets.is_empty() && !body.is_empty() {
        out.push_str("\nRe-run with --verbose for the full response body.");
    }

    out
}

#[derive(Default)]
struct ParsedApiBody {
    bullets: Vec<String>,
    hints: Vec<String>,
}

/// Parses a JSON-shaped API error body, extracting human-readable messages and
/// (when the body contains "must be one of …" enum lists) appending
/// did-you-mean suggestions against the user's argv.
fn parse_api_body(body: &str, argv: &[String]) -> ParsedApiBody {
    let mut out = ParsedApiBody::default();
    if body.is_empty() {
        return out;
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return out;
    };

    let mut raw_strings: Vec<String> = Vec::new();
    collect_error_strings(&value, &mut raw_strings);
    if raw_strings.is_empty() {
        return out;
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut fields_hinted: BTreeSet<String> = BTreeSet::new();

    for s in raw_strings {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
            continue;
        }
        if is_generic_label(&trimmed) {
            // Skip "Bad Request" / "Unauthorized" — these duplicate the headline.
            continue;
        }
        let bullet = decorate_with_suggestion(&trimmed, argv, &mut fields_hinted);
        out.bullets.push(bullet);
    }

    for field in &fields_hinted {
        if let Some(hint) = field_hint(field) {
            out.hints.push(hint.to_string());
        }
    }

    out
}

/// Recursively walk a JSON value, pulling strings out of any key named
/// `error`, `errors`, `message`, or `messages`. Accepts strings, arrays of
/// strings, arrays of objects (recurse), and nested objects (recurse).
fn collect_error_strings(value: &Value, out: &mut Vec<String>) {
    const KEYS: &[&str] = &["errors", "error", "messages", "message"];
    match value {
        Value::Object(map) => {
            for key in KEYS {
                if let Some(v) = map.get(*key) {
                    collect_strings_from(v, out);
                }
            }
            // Also recurse into other object values so we can find nested
            // error/message keys (e.g. NestJS wraps under `message.message`).
            for (k, v) in map {
                if !KEYS.contains(&k.as_str()) {
                    collect_error_strings(v, out);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_error_strings(v, out);
            }
        }
        _ => {}
    }
}

/// Helper: when we hit one of the error keys, accept multiple shapes.
fn collect_strings_from(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(arr) => {
            for v in arr {
                match v {
                    Value::String(s) => out.push(s.clone()),
                    Value::Object(_) => collect_error_strings(v, out),
                    _ => {}
                }
            }
        }
        Value::Object(_) => collect_error_strings(value, out),
        _ => {}
    }
}

/// Returns the bullet text, possibly with a `did you mean '…'?` suffix and a
/// `(N more)` truncation marker. Also records which fields had enum lists so
/// the caller can attach helper hints.
fn decorate_with_suggestion(
    raw: &str,
    argv: &[String],
    fields_hinted: &mut BTreeSet<String>,
) -> String {
    let Some((field, candidates)) = parse_must_be_one_of(raw) else {
        return raw.to_string();
    };

    fields_hinted.insert(field.clone());

    // Find the argv value that's closest to any candidate, then attach DYM if
    // the best match is within threshold.
    let best = best_suggestion(argv, &candidates);

    let display = truncate_candidate_list(&candidates, 5);
    let mut bullet = format!("{field} must be one of: {display}");
    if let Some((user_value, suggestion)) = best {
        bullet.push_str(&format!(
            " — did you mean '{suggestion}' (you passed '{user_value}')?"
        ));
    }
    bullet
}

/// Parse `"<field> must be one of [the following values:] X, Y, Z"`.
/// Returns the field name and the candidate list.
fn parse_must_be_one_of(s: &str) -> Option<(String, Vec<String>)> {
    let (field_part, rest) = s.split_once(" must be one of")?;
    let field = field_part.trim();
    if field.is_empty()
        || !field
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    // After "must be one of" we accept any of: " the following values: X, Y",
    // ": X, Y", or " X, Y" (rare but possible).
    let list_part = rest
        .strip_prefix(" the following values: ")
        .or_else(|| rest.strip_prefix(": "))
        .or_else(|| rest.strip_prefix(' '))
        .unwrap_or(rest)
        .trim_end_matches('.');
    let candidates: Vec<String> = list_part
        .split(", ")
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if candidates.len() < 2 {
        return None;
    }
    Some((field.to_string(), candidates))
}

/// Find the (argv-value, candidate) pair with smallest Levenshtein distance,
/// gated on: distance ≤ 3 AND ≥ 3 leading chars shared with the candidate.
fn best_suggestion(argv: &[String], candidates: &[String]) -> Option<(String, String)> {
    let mut best: Option<(usize, String, String)> = None;
    for arg in argv {
        // Skip flags themselves and obviously-non-value tokens.
        if arg.starts_with('-') || arg.is_empty() || arg.len() < 2 {
            continue;
        }
        for cand in candidates {
            let d = levenshtein(arg, cand);
            if d > 3 {
                continue;
            }
            if shared_prefix_len(arg, cand) < 3 {
                continue;
            }
            match best.as_ref() {
                None => best = Some((d, arg.clone(), cand.clone())),
                Some((cur, _, _)) if d < *cur => best = Some((d, arg.clone(), cand.clone())),
                _ => {}
            }
        }
    }
    best.map(|(_, a, c)| (a, c))
}

fn shared_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// Classic O(n*m) Levenshtein distance. n,m are tiny here (≤ ~40 chars), so
/// this is fine.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn truncate_candidate_list(candidates: &[String], keep: usize) -> String {
    if candidates.len() <= keep {
        return candidates.join(", ");
    }
    let shown = candidates[..keep].join(", ");
    let extra = candidates.len() - keep;
    format!("{shown} ({extra} more)")
}

/// Maps known server-side field names to a follow-up command the user can run
/// to discover valid values.
/// Skip standard HTTP status-phrase strings that duplicate the headline.
fn is_generic_label(s: &str) -> bool {
    matches!(
        s,
        "Bad Request"
            | "Unauthorized"
            | "Forbidden"
            | "Not Found"
            | "Unprocessable Entity"
            | "Too Many Requests"
            | "Internal Server Error"
            | "Service Unavailable"
    )
}

fn field_hint(field: &str) -> Option<&'static str> {
    match field {
        "network" => Some("Run 'qn chain list' to see supported networks."),
        "chain" => Some("Run 'qn chain list' to see supported chains."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quicknode_sdk::errors::SdkError;

    fn api_err_with(code: u16, body: &str) -> CliError {
        CliError::Sdk(SdkError::Api {
            status: reqwest::StatusCode::from_u16(code).unwrap(),
            body: body.to_string(),
        })
    }

    fn api_err(code: u16) -> CliError {
        api_err_with(code, "{\"message\":\"boom\"}")
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

    // ---- payment errors ----

    fn decode_err() -> SdkError {
        SdkError::Decode {
            source: serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
            body: "<html>gateway oops</html>".to_string(),
        }
    }

    #[test]
    fn exit_code_payment_refusals_are_2() {
        // The gateway said no and nothing settled: an unmatched/unreadable
        // offer (unsupported) or a 4xx-refused credential (rejected). The
        // paid lane wraps 5xx rejections into PaymentMaybeCharged before
        // this mapping ever sees them.
        let unsupported = CliError::Sdk(SdkError::PaymentUnsupported {
            offered: "eip155:84532/0xabc amount 999999".to_string(),
        });
        let rejected = CliError::Sdk(SdkError::PaymentRejected {
            status: 402,
            body: "invalid signature".to_string(),
        });
        assert_eq!(exit_code_for(&unsupported), 2);
        assert_eq!(exit_code_for(&rejected), 2);
    }

    #[test]
    fn exit_code_unknown_payment_outcome_is_3() {
        // Request sent, outcome unknown: the transport-ambiguity bucket, so
        // scripts can distinguish "safe to retry" (2) from "check wallet" (3).
        let indeterminate = CliError::Sdk(SdkError::PaymentIndeterminate);
        let maybe_charged = CliError::PaymentMaybeCharged(decode_err());
        assert_eq!(exit_code_for(&indeterminate), 3);
        assert_eq!(exit_code_for(&maybe_charged), 3);
    }

    #[test]
    fn renders_payment_unsupported_as_not_charged() {
        let err = CliError::Sdk(SdkError::PaymentUnsupported {
            offered: "eip155:84532/0xabc amount 999999".to_string(),
        });
        let msg = render(&err, false);
        assert!(msg.contains("Nothing was charged"), "got: {msg}");
        assert!(msg.contains("999999"), "got: {msg}");
        assert!(msg.contains("--max-amount"), "got: {msg}");
    }

    #[test]
    fn renders_payment_rejected_as_refused_without_settling() {
        // A short gateway reason surfaces in the default message (the SDK has
        // already reduced its JSON error shape to this string).
        let err = CliError::Sdk(SdkError::PaymentRejected {
            status: 402,
            body: "insufficient funds".to_string(),
        });
        let msg = render(&err, false);
        assert!(msg.contains("402"), "got: {msg}");
        assert!(msg.contains("refused"), "got: {msg}");
        assert!(msg.contains("nothing should have settled"), "got: {msg}");
        assert!(msg.contains("Gateway: insufficient funds"), "got: {msg}");
    }

    #[test]
    fn payment_rejected_hides_long_body_unless_verbose() {
        // A long/JSON body (e.g. a full 402 payment menu) is not a reason: the
        // default message stays generic and the raw body appears under --verbose.
        let body = format!("{{\"accepts\":[{}]}}", "\"x\",".repeat(80));
        let err = CliError::Sdk(SdkError::PaymentRejected {
            status: 402,
            body: body.clone(),
        });
        let msg = render(&err, false);
        assert!(
            !msg.contains(&body),
            "long body must not leak by default: {msg}"
        );
        assert!(msg.contains("qn rpc pay-networks"), "got: {msg}");
        let verbose = render(&err, true);
        assert!(verbose.contains("accepts"), "got: {verbose}");
    }

    #[test]
    fn renders_payment_indeterminate_as_possibly_settled() {
        let msg = render(&CliError::Sdk(SdkError::PaymentIndeterminate), false);
        assert!(msg.contains("may have been settled"), "got: {msg}");
        assert!(msg.contains("check your wallet"), "got: {msg}");
    }

    #[test]
    fn renders_payment_maybe_charged_with_source_when_verbose() {
        let err = CliError::PaymentMaybeCharged(decode_err());
        let msg = render(&err, false);
        assert!(msg.contains("may have been settled"), "got: {msg}");
        assert!(!msg.contains("gateway oops"), "got: {msg}");
        let verbose = render(&err, true);
        assert!(verbose.contains("gateway oops"), "got: {verbose}");
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

    // ---- body parsing ----

    #[test]
    fn nestjs_shape_extracts_bullets() {
        let body = r#"{"statusCode":400,"message":{"message":["network must be one of the following values: ethereum-mainnet, ethereum-sepolia, solana-mainnet","status must be one of the following values: active, paused, terminated"],"error":"Bad Request"}}"#;
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.starts_with("Error: invalid request."), "got: {msg}");
        assert!(msg.contains("• network must be one of:"), "got: {msg}");
        assert!(msg.contains("• status must be one of:"), "got: {msg}");
    }

    #[test]
    fn admin_shape_extracts_error_string() {
        let body = r#"{"data":null,"error":"undefined method `chain' for nil"}"#;
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.contains("undefined method"), "got: {msg}");
    }

    #[test]
    fn empty_body_400_falls_through() {
        let msg = render(&api_err_with(400, ""), false);
        assert_eq!(msg, "Error: invalid request.");
    }

    #[test]
    fn garbage_non_json_body_falls_back_to_raw() {
        let body = "<html>oops</html>";
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.contains("<html>oops</html>"), "got: {msg}");
    }

    #[test]
    fn generic_errors_array_of_strings() {
        let body = r#"{"errors":["first thing wrong","second thing wrong"]}"#;
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.contains("• first thing wrong"), "got: {msg}");
        assert!(msg.contains("• second thing wrong"), "got: {msg}");
    }

    #[test]
    fn generic_errors_array_of_objects() {
        let body = r#"{"errors":[{"message":"thing one"},{"message":"thing two"}]}"#;
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.contains("• thing one"), "got: {msg}");
        assert!(msg.contains("• thing two"), "got: {msg}");
    }

    #[test]
    fn dedupes_repeated_strings() {
        let body = r#"{"error":"same thing","message":"same thing"}"#;
        let msg = render(&api_err_with(400, body), false);
        let count = msg.matches("same thing").count();
        assert_eq!(count, 1, "expected dedupe, got: {msg}");
    }

    #[test]
    fn truncates_long_enum_list() {
        // 10 candidates, only the first 5 should render inline.
        let body = r#"{"message":"x must be one of a, b, c, d, e, f, g, h, i, j"}"#;
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.contains("a, b, c, d, e (5 more)"), "got: {msg}");
    }

    #[test]
    fn field_hint_appended_for_network() {
        let body = r#"{"message":"network must be one of: ethereum-mainnet, solana-mainnet"}"#;
        let msg = render(&api_err_with(400, body), false);
        assert!(msg.contains("qn chain list"), "got: {msg}");
    }

    #[test]
    fn verbose_appends_full_body() {
        let body = r#"{"message":["network must be one of: a, b, c"]}"#;
        let msg = render(&api_err_with(400, body), true);
        assert!(msg.contains(body), "got: {msg}");
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("a", ""), 1);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("ethereum-mainnet", "ethereum-mainnet"), 0);
        assert_eq!(levenshtein("ethereum-mainnetsds", "ethereum-mainnet"), 3);
    }

    #[test]
    fn parse_must_be_one_of_happy_path() {
        let (f, c) =
            parse_must_be_one_of("network must be one of the following values: a, b, c").unwrap();
        assert_eq!(f, "network");
        assert_eq!(c, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_must_be_one_of_no_following_values_prefix() {
        let (f, c) = parse_must_be_one_of("status must be one of active, paused").unwrap();
        assert_eq!(f, "status");
        assert_eq!(c, vec!["active", "paused"]);
    }

    #[test]
    fn parse_must_be_one_of_rejects_unrelated_strings() {
        assert!(parse_must_be_one_of("some random error").is_none());
    }

    #[test]
    fn truncate_candidate_list_under_keep_returns_all() {
        assert_eq!(
            truncate_candidate_list(&["a".into(), "b".into()], 5),
            "a, b"
        );
    }

    #[test]
    fn best_suggestion_picks_closest_within_threshold() {
        let candidates: Vec<String> =
            vec!["ethereum-mainnet", "ethereum-sepolia", "solana-mainnet"]
                .into_iter()
                .map(String::from)
                .collect();
        let argv = vec!["ethereum-mainnetsds".to_string()];
        let suggestion = best_suggestion(&argv, &candidates);
        assert_eq!(
            suggestion,
            Some(("ethereum-mainnetsds".into(), "ethereum-mainnet".into()))
        );
    }

    #[test]
    fn best_suggestion_returns_none_if_too_far() {
        let candidates: Vec<String> = vec!["ethereum-mainnet"]
            .into_iter()
            .map(String::from)
            .collect();
        let argv = vec!["sfjla".to_string()];
        assert_eq!(best_suggestion(&argv, &candidates), None);
    }

    #[test]
    fn best_suggestion_ignores_flag_tokens() {
        let candidates: Vec<String> = vec!["chain"].into_iter().map(String::from).collect();
        let argv = vec!["--chain".to_string()];
        // "--chain" starts with "-", should be skipped.
        assert_eq!(best_suggestion(&argv, &candidates), None);
    }

    #[test]
    fn renders_5xx_skips_body_parsing() {
        // We don't want stack-trace HTML on a 500 to be parsed as bullets.
        let body = r#"{"message":"internal error"}"#;
        let msg = render(&api_err_with(500, body), false);
        assert!(!msg.contains("•"), "got: {msg}");
    }
}
