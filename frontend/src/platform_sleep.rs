//! Shared platform-gated async sleep: web awaits `gloo_timers`, every other
//! build (server SSR compile, mobile native) awaits `tokio::time`.

/// Sleep for `ms` milliseconds, yielding to the browser event loop on web or
/// the tokio runtime elsewhere.
#[cfg(feature = "web")]
pub async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(not(feature = "web"))]
pub async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
}

#[cfg(all(test, not(feature = "web")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn async_sleep_ms_waits_at_least_the_requested_duration() {
        let start = std::time::Instant::now();
        async_sleep_ms(10).await;
        assert!(start.elapsed().as_millis() >= 10);
    }
}
