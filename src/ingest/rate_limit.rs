//! Shared provider rate limiters for ingestion adapters.
//!
//! Per-service sleeps are not enough when several adapters hit the same vendor
//! concurrently. These process-wide limiters serialize calls per provider and
//! extend the next allowed request time when a 429 is observed.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::StatusCode;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::warn;

#[derive(Debug)]
struct State {
    next_allowed: Instant,
    backoff: Duration,
}

#[derive(Debug)]
pub struct ProviderLimiter {
    name: &'static str,
    min_delay: Duration,
    default_backoff: Duration,
    max_backoff: Duration,
    state: Mutex<State>,
}

impl ProviderLimiter {
    fn new(
        name: &'static str,
        min_delay: Duration,
        default_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        Self {
            name,
            min_delay,
            default_backoff,
            max_backoff,
            state: Mutex::new(State {
                next_allowed: Instant::now(),
                backoff: Duration::ZERO,
            }),
        }
    }

    pub async fn wait(&self) {
        loop {
            let sleep_for = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                if now >= state.next_allowed {
                    state.next_allowed = now + self.min_delay;
                    None
                } else {
                    Some(state.next_allowed - now)
                }
            };
            let Some(sleep_for) = sleep_for else { return };
            tokio::time::sleep(sleep_for).await;
        }
    }

    pub async fn observe_status(&self, status: StatusCode, retry_after: Option<Duration>) {
        if status == StatusCode::TOO_MANY_REQUESTS {
            self.backoff(retry_after).await;
        } else if status.is_success() {
            let mut state = self.state.lock().await;
            state.backoff = Duration::ZERO;
        }
    }

    async fn backoff(&self, retry_after: Option<Duration>) {
        let mut state = self.state.lock().await;
        let computed = if state.backoff.is_zero() {
            self.default_backoff
        } else {
            state.backoff.saturating_mul(2).min(self.max_backoff)
        };
        let wait = retry_after.unwrap_or(computed).min(self.max_backoff);
        state.backoff = wait;
        state.next_allowed = state.next_allowed.max(Instant::now() + wait);
        warn!(
            provider = self.name,
            backoff_secs = wait.as_secs_f32(),
            "provider rate limited; backing off"
        );
    }
}

fn env_duration_ms(name: &str, default_ms: u64) -> Duration {
    let ms = std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default_ms);
    Duration::from_millis(ms)
}

pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    raw.parse::<u64>().ok().map(Duration::from_secs)
}

pub fn fmp() -> &'static ProviderLimiter {
    static LIMITER: OnceLock<ProviderLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| {
        ProviderLimiter::new(
            "fmp",
            env_duration_ms("FMP_MIN_REQUEST_INTERVAL_MS", 750),
            env_duration_ms("FMP_RATE_LIMIT_BACKOFF_MS", 60_000),
            env_duration_ms("FMP_MAX_RATE_LIMIT_BACKOFF_MS", 15 * 60_000),
        )
    })
}

pub fn fred() -> &'static ProviderLimiter {
    static LIMITER: OnceLock<ProviderLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| {
        ProviderLimiter::new(
            "fred",
            env_duration_ms("FRED_MIN_REQUEST_INTERVAL_MS", 2_000),
            env_duration_ms("FRED_RATE_LIMIT_BACKOFF_MS", 5 * 60_000),
            env_duration_ms("FRED_MAX_RATE_LIMIT_BACKOFF_MS", 60 * 60_000),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parses_seconds_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "42".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(42)));
    }

    #[test]
    fn retry_after_ignores_unparseable_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&headers), None);
    }

    #[tokio::test]
    async fn repeated_429s_escalate_until_success_resets() {
        let limiter = ProviderLimiter::new(
            "test",
            Duration::from_millis(1),
            Duration::from_secs(10),
            Duration::from_secs(40),
        );

        limiter
            .observe_status(StatusCode::TOO_MANY_REQUESTS, None)
            .await;
        assert_eq!(limiter.state.lock().await.backoff, Duration::from_secs(10));

        limiter
            .observe_status(StatusCode::TOO_MANY_REQUESTS, None)
            .await;
        assert_eq!(limiter.state.lock().await.backoff, Duration::from_secs(20));

        limiter
            .observe_status(StatusCode::TOO_MANY_REQUESTS, None)
            .await;
        assert_eq!(limiter.state.lock().await.backoff, Duration::from_secs(40));

        limiter.observe_status(StatusCode::OK, None).await;
        assert_eq!(limiter.state.lock().await.backoff, Duration::ZERO);
    }

    #[tokio::test]
    async fn retry_after_is_capped_by_max_backoff() {
        let limiter = ProviderLimiter::new(
            "test",
            Duration::from_millis(1),
            Duration::from_secs(10),
            Duration::from_secs(40),
        );

        limiter
            .observe_status(
                StatusCode::TOO_MANY_REQUESTS,
                Some(Duration::from_secs(120)),
            )
            .await;

        assert_eq!(limiter.state.lock().await.backoff, Duration::from_secs(40));
    }
}
