//! Shared platform-gated async sleep: web awaits `gloo_timers`, the server
//! and mobile builds (the only other targets `tokio` is enabled for) await
//! `tokio::time`. A no-feature build has neither, so it gets a no-op stub.

/// Sleep for `ms` milliseconds, yielding to the browser event loop on web or
/// the tokio runtime on server/mobile.
#[cfg(feature = "web")]
pub async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

// `tokio` is an optional dep enabled only by `server` and `mobile` — gate
// on those explicitly rather than a bare `not(web)`, which would try to
// link `tokio` in a plain no-feature build and fail.
#[cfg(all(not(feature = "web"), any(feature = "server", feature = "mobile")))]
pub async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
}

/// No-feature stub: neither `gloo_timers` nor `tokio` is available, and
/// nothing calls this helper on that target anyway.
#[cfg(not(any(feature = "web", feature = "server", feature = "mobile")))]
pub async fn async_sleep_ms(_ms: u32) {}

#[cfg(all(
    test,
    not(feature = "web"),
    any(feature = "server", feature = "mobile")
))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn async_sleep_ms_waits_at_least_the_requested_duration() {
        let start = std::time::Instant::now();
        async_sleep_ms(10).await;
        assert!(start.elapsed().as_millis() >= 10);
    }
}
