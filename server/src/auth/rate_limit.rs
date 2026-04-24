//! Minimal in-memory per-IP rate limiter for the login/register endpoints.
//!
//! Self-hosted scope is small — a single process, ≤ a few dozen users. A
//! Redis-backed limiter is overkill; a `Mutex<HashMap<IpAddr, Bucket>>` fits
//! in < 60 lines and is easy to reason about. Swap for `tower_governor` if
//! the scope ever grows.
//!
//! Policy: fixed-window counter, `MAX_REQUESTS` per `WINDOW_SECS`. Default
//! tuned for `/api/auth/login` and `/api/auth/register`: 10 requests /
//! minute / IP.

use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const WINDOW_SECS: u64 = 60;
pub const MAX_REQUESTS: u32 = 10;
/// Maximum number of tracked IPs before stale buckets are pruned.
const MAX_BUCKETS: usize = 10_000;

fn trust_forwarded_for() -> bool {
    matches!(
        std::env::var("OMNIBUS_TRUST_FORWARDED_FOR").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

struct Bucket {
    window_start: Instant,
    count: u32,
}

/// `tokio::sync::Mutex` is used here rather than `std::sync::Mutex` so that
/// `allow()` never blocks a Tokio worker thread while waiting for the lock
/// under contention.
pub struct RateLimiter {
    inner: Mutex<HashMap<IpAddr, Bucket>>,
    window: Duration,
    max: u32,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::with_policy(Duration::from_secs(WINDOW_SECS), MAX_REQUESTS)
    }

    pub fn with_policy(window: Duration, max: u32) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            window,
            max,
        }
    }

    pub async fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.inner.lock().await;

        // Prune stale entries when the map gets large to prevent unbounded growth.
        if map.len() >= MAX_BUCKETS {
            let window = self.window;
            map.retain(|_, b| now.duration_since(b.window_start) < window * 2);
        }

        let bucket = map.entry(ip).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= self.window {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= self.max {
            return false;
        }
        bucket.count += 1;
        true
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum middleware scoping the limiter to `/api/auth/login` and
/// `/api/auth/register`. Prefers `ConnectInfo<SocketAddr>` (wired by the
/// server's make-service). Only consults `X-Forwarded-For` when the operator
/// has opted in via `OMNIBUS_TRUST_FORWARDED_FOR=1` — otherwise a client on
/// a directly-reachable deployment could spoof the header to bypass the
/// limiter and grow the bucket map without bound. When neither source yields
/// an IP, falls back to `0.0.0.0` so the limiter still applies process-wide.
pub async fn rate_limit_auth(
    State(limiter): State<Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let targeted = matches!(path, "/api/auth/login" | "/api/auth/register");
    if !targeted {
        return next.run(req).await;
    }
    let direct = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(a)| a.ip());
    let ip = direct
        .or_else(|| {
            if !trust_forwarded_for() {
                return None;
            }
            req.headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse().ok())
        })
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    if !limiter.allow(ip).await {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_up_to_max_then_blocks() {
        let rl = RateLimiter::with_policy(Duration::from_secs(60), 3);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(rl.allow(ip).await);
        assert!(rl.allow(ip).await);
        assert!(rl.allow(ip).await);
        assert!(!rl.allow(ip).await);
    }

    #[tokio::test]
    async fn separate_ips_have_separate_buckets() {
        let rl = RateLimiter::with_policy(Duration::from_secs(60), 1);
        let a: IpAddr = "127.0.0.1".parse().unwrap();
        let b: IpAddr = "127.0.0.2".parse().unwrap();
        assert!(rl.allow(a).await);
        assert!(!rl.allow(a).await);
        assert!(rl.allow(b).await);
    }

    #[tokio::test]
    async fn window_resets_after_elapsed() {
        let rl = RateLimiter::with_policy(Duration::from_millis(10), 1);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(rl.allow(ip).await);
        assert!(!rl.allow(ip).await);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(rl.allow(ip).await);
    }

    #[tokio::test]
    async fn prunes_stale_entries_at_cap() {
        let rl = RateLimiter::with_policy(Duration::from_millis(1), MAX_REQUESTS);
        // Fill to just under the cap using distinct IPs.
        for i in 0..MAX_BUCKETS {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(i as u32));
            rl.allow(ip).await;
        }
        // All windows are stale; a new allow() call should prune and succeed.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(rl.allow(ip).await);
        assert!(rl.inner.lock().await.len() < MAX_BUCKETS);
    }
}
