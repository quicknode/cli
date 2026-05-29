//! Parses CLI time arguments (`--from`, `--to`).
//!
//! Accepts:
//! - the literal `now`
//! - relative durations parsed by `humantime` (e.g. `1h`, `7d`, `30m`, `2w`) →
//!   interpreted as "that long ago"
//! - ISO-8601 / RFC-3339 timestamps (e.g. `2026-04-01T00:00:00Z`)
//!
//! Outputs:
//! - [`to_unix`] returns a Unix timestamp (seconds) — used by the usage endpoints.
//! - [`to_rfc3339`] returns an RFC-3339 string — used by the log endpoints.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::errors::CliError;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy)]
pub struct ParsedTime(pub OffsetDateTime);

impl ParsedTime {
    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    pub fn to_unix(self) -> i64 {
        self.0.unix_timestamp()
    }

    pub fn to_rfc3339(self) -> String {
        self.0
            .format(&Rfc3339)
            .expect("rfc3339 format is always valid")
    }
}

pub fn parse(input: &str) -> Result<ParsedTime, CliError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(CliError::Arg("empty time value".to_string()));
    }
    if input.eq_ignore_ascii_case("now") {
        return Ok(ParsedTime::now());
    }

    if let Ok(duration) = humantime::parse_duration(input) {
        let now = SystemTime::now();
        let then = now
            .checked_sub(duration)
            .ok_or_else(|| CliError::Arg(format!("relative time {input:?} underflows")))?;
        let secs = then
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CliError::Arg(format!("relative time {input:?} before unix epoch")))?
            .as_secs() as i64;
        let dt = OffsetDateTime::from_unix_timestamp(secs)
            .map_err(|e| CliError::Arg(format!("internal time conversion: {e}")))?;
        return Ok(ParsedTime(dt));
    }

    if let Ok(dt) = OffsetDateTime::parse(input, &Rfc3339) {
        return Ok(ParsedTime(dt));
    }

    Err(CliError::Arg(format!(
        "could not parse time {input:?}. Try 'now', a duration like '7d', or an RFC-3339 timestamp."
    )))
}

/// Convenience: same as [`parse`] but only the unix-timestamp result.
pub fn parse_unix(input: &str) -> Result<i64, CliError> {
    parse(input).map(|p| p.to_unix())
}

/// Convenience: same as [`parse`] but only the RFC-3339 string.
pub fn parse_rfc3339(input: &str) -> Result<String, CliError> {
    parse(input).map(|p| p.to_rfc3339())
}

// Default for time::Duration not stable yet; provide a helper that yields a
// SystemTime offset.
fn _unused(_: Duration) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_now() {
        let n = parse("now").unwrap();
        let diff = OffsetDateTime::now_utc().unix_timestamp() - n.to_unix();
        assert!(diff.abs() < 2, "now is now-ish, got diff {diff}");
    }

    #[test]
    fn parses_now_case_insensitive() {
        parse("NOW").unwrap();
        parse("Now").unwrap();
    }

    #[test]
    fn parses_relative_duration() {
        let then = parse("1h").unwrap();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        // ~3600 seconds ago, allow a couple seconds of slack
        let diff = now - then.to_unix();
        assert!((3598..=3602).contains(&diff), "expected ~3600s, got {diff}");
    }

    #[test]
    fn parses_iso8601() {
        let dt = parse("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn rejects_garbage() {
        let err = parse("definitely not a time").unwrap_err();
        assert!(matches!(err, CliError::Arg(_)));
    }

    #[test]
    fn rejects_empty() {
        let err = parse("   ").unwrap_err();
        assert!(matches!(err, CliError::Arg(_)));
    }
}
