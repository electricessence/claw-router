//! Token-bucket rate limiting middleware.
//!
//! One bucket per client IP. Tokens refill steadily at `rpm / 60` tokens/second
//! and the burst cap is `ceil(rpm / 2)` — enough to absorb short spikes without
//! allowing runaway bursts. Rate limiting is disabled when `rate_limit_rpm` is
//! absent from the gateway config.
//!
//! When a request is rejected the response includes:
//! - `429 Too Many Requests`
//! - `Retry-After: <seconds>` — exact wait before the bucket has a token again
//! - `X-RateLimit-Limit: <rpm>` — configured limit
//! - `X-RateLimit-Policy: <N>;w=60` — standard hint: N requests per 60-second window

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Instant,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;

use crate::router::RouterState;

/// Per-IP token bucket state.
#[derive(Debug, Clone)]
struct Bucket {
    /// Timestamp of the last time tokens were refilled.
    last_refill: Instant,
    /// Current token count (fractional to avoid drift).
    tokens: f64,
}

/// Shared rate limiter: one token bucket per client IP address.
pub struct RateLimiter {
    /// Configured limit in requests per minute.
    pub rpm: u32,
    /// Token refill rate (tokens / second = rpm / 60).
    fill_rate: f64,
    /// Maximum bucket capacity (burst allowance = ceil(rpm / 2)).
    capacity: f64,
    /// Per-IP bucket state.
    buckets: DashMap<IpAddr, Bucket>,
}

impl RateLimiter {
    /// Create a new rate limiter for the given requests-per-minute limit.
    pub fn new(rpm: u32) -> Self {
        let capacity = ((rpm + 1) / 2) as f64; // ceil(rpm / 2)
        let fill_rate = rpm as f64 / 60.0;
        Self {
            rpm,
            fill_rate,
            capacity,
            buckets: DashMap::new(),
        }
    }

    /// Attempt to consume one token for `ip`.
    ///
    /// Returns `Ok(())` if the request is allowed, or `Err(retry_after_secs)`
    /// if the bucket is empty.
    pub fn check(&self, ip: IpAddr) -> Result<(), f64> {
        let now = Instant::now();

        let mut bucket = self.buckets.entry(ip).or_insert_with(|| Bucket {
            last_refill: now,
            tokens: self.capacity,
        });

        // Refill tokens based on elapsed time.
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        let new_tokens = (bucket.tokens + elapsed * self.fill_rate).min(self.capacity);

        if new_tokens < 1.0 {
            // Compute how long until the bucket has a full token.
            let retry_after = (1.0 - new_tokens) / self.fill_rate;
            return Err(retry_after.ceil());
        }

        bucket.last_refill = now;
        bucket.tokens = new_tokens - 1.0;
        Ok(())
    }
}

/// Axum middleware that enforces per-IP rate limits.
///
/// No-ops (passes through) when `state.rate_limiter` is `None`.
/// Falls back to `127.0.0.1` if `ConnectInfo` is unavailable (e.g., in tests).
pub async fn rate_limit_middleware(
    State(state): State<Arc<RouterState>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(limiter) = &state.rate_limiter {
        // Read the peer address from extensions — set by into_make_service_with_connect_info.
        let ip = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

        if let Err(retry_after) = limiter.check(ip) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    ("retry-after", retry_after.to_string()),
                    ("x-ratelimit-limit", limiter.rpm.to_string()),
                    ("x-ratelimit-policy", format!("{};w=60", limiter.rpm)),
                    ("content-type", "text/plain".into()),
                ],
                "Rate limit exceeded. Please retry after the indicated delay.",
            )
                .into_response();
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(a: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, a))
    }

    #[test]
    fn fresh_bucket_allows_up_to_capacity() {
        let limiter = RateLimiter::new(60); // capacity = 30
        let test_ip = ip(1);

        // Should allow up to capacity (30) requests immediately
        let allowed = (0..limiter.capacity as usize)
            .filter(|_| limiter.check(test_ip).is_ok())
            .count();

        assert_eq!(allowed, limiter.capacity as usize, "expected {capacity} immediate requests", capacity = limiter.capacity as usize);
    }

    #[test]
    fn exceeding_capacity_returns_retry_after() {
        let limiter = RateLimiter::new(60); // capacity = 30, fill_rate = 1 token/sec
        let test_ip = ip(2);

        // Drain the bucket
        for _ in 0..limiter.capacity as usize {
            let _ = limiter.check(test_ip);
        }

        // Next request should be rate-limited
        let result = limiter.check(test_ip);
        assert!(result.is_err(), "bucket should be exhausted");
        let retry = result.unwrap_err();
        assert!(retry >= 1.0, "retry_after must be at least 1 second");
    }

    #[test]
    fn different_ips_have_independent_buckets() {
        let limiter = RateLimiter::new(4); // capacity = 2
        let ip_a = ip(10);
        let ip_b = ip(11);

        // Drain ip_a
        let _ = limiter.check(ip_a);
        let _ = limiter.check(ip_a);

        // ip_b should still have a full bucket
        assert!(limiter.check(ip_b).is_ok(), "ip_b should be unaffected by ip_a");
    }
}
