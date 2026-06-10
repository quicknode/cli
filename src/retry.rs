//! Full-jitter exponential backoff for read-only API calls.
//!
//! Mutating commands must NOT use this: a retried create/update/delete can be
//! applied twice (duplicate endpoints, double tag writes, …). Reads are safe
//! to repeat, and the backend is rate-limited, so scripted reads need to
//! survive the occasional 429.
//!
//! Backoff is driven purely by full jitter (`SdkError::Api` exposes the
//! status and body, which is what the retry decision is based on); the
//! randomized window keeps concurrent callers from herding.

use std::future::Future;
use std::time::Duration;

use quicknode_sdk::errors::{HttpKind, SdkError};

const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 8_000;

/// Run `f`, retrying up to `max_retries` extra times on transient failures
/// (timeouts, connect errors, HTTP 429/500/502/504/503). `f` is invoked fresh
/// for each attempt. `max_retries = 0` means a single attempt, no retry.
pub async fn retrying<T, F, Fut>(max_retries: u32, f: F) -> Result<T, SdkError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, SdkError>>,
{
    let mut attempt: u32 = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt < max_retries && is_retryable(&e) => {
                tokio::time::sleep(delay_for(attempt)).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Transient failures only. Auth errors, 404s, validation errors, etc. will
/// fail identically on every attempt — retrying them just adds latency.
fn is_retryable(e: &SdkError) -> bool {
    match e {
        SdkError::Api { status, .. } => matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504),
        SdkError::Http(_) => matches!(e.http_kind(), Some(HttpKind::Timeout | HttpKind::Connect)),
        _ => false,
    }
}

/// Full jitter: a uniformly random delay in `[0, base * 2^attempt]`, capped.
/// Randomizing the whole interval (rather than adding a small jitter term)
/// spreads concurrent retriers across the window instead of re-synchronizing
/// them at the next power of two.
fn delay_for(attempt: u32) -> Duration {
    let exp = attempt.min(10); // 2^10 * 500ms is already far past the cap
    let ceiling_ms = BASE_DELAY_MS.saturating_mul(1 << exp).min(MAX_DELAY_MS);
    Duration::from_millis(fastrand::u64(0..=ceiling_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn api_error(status: u16) -> SdkError {
        SdkError::Api {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            body: String::new(),
        }
    }

    #[test]
    fn retryable_statuses() {
        for s in [429, 500, 502, 503, 504] {
            assert!(is_retryable(&api_error(s)), "{s} should be retryable");
        }
        for s in [400, 401, 403, 404, 422] {
            assert!(!is_retryable(&api_error(s)), "{s} should not be retryable");
        }
    }

    #[test]
    fn delay_never_exceeds_cap() {
        for attempt in 0..64 {
            assert!(delay_for(attempt) <= Duration::from_millis(MAX_DELAY_MS));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retries_until_success() {
        let calls = AtomicU32::new(0);
        let result = retrying(3, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(api_error(429))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn exhausts_retries_and_returns_last_error() {
        let calls = AtomicU32::new(0);
        let result: Result<(), _> = retrying(2, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(api_error(503)) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3); // 1 attempt + 2 retries
    }

    #[tokio::test(start_paused = true)]
    async fn non_retryable_fails_on_first_attempt() {
        let calls = AtomicU32::new(0);
        let result: Result<(), _> = retrying(3, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(api_error(404)) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_retries_means_single_attempt() {
        let calls = AtomicU32::new(0);
        let result: Result<(), _> = retrying(0, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(api_error(429)) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
