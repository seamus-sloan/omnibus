//! Client-side wall-clock time: the JS clock on web, `SystemTime` elsewhere.
//! Used for optimistic-write timestamps and relative-time labels that only
//! ever run client-side (SSR never invokes the JS clock, so there's no
//! cross-target consistency requirement to worry about here).

/// Current unix time in seconds.
pub fn now_unix() -> i64 {
    #[cfg(feature = "web")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(feature = "web"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Viewer's local UTC offset in seconds (positive east of UTC), read
/// synchronously from the browser's `Date.getTimezoneOffset` on web. Zero
/// everywhere else: SSR has no browser to ask, and mobile's `SystemTime`
/// clock carries no zone information.
pub fn local_utc_offset_secs() -> i64 {
    #[cfg(feature = "web")]
    {
        // JS convention: `getTimezoneOffset` returns (UTC - local) in
        // minutes, so local = UTC + (-offset_minutes * 60).
        (-js_sys::Date::new_0().get_timezone_offset() * 60.0) as i64
    }
    #[cfg(not(feature = "web"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_unix_returns_a_plausible_recent_timestamp() {
        // Any unix-seconds value after 2020-01-01T00:00:00Z.
        assert!(now_unix() > 1_577_836_800);
    }

    #[test]
    fn local_utc_offset_secs_is_zero_off_web() {
        // This test target has no browser to ask, so the non-web fallback
        // must report UTC rather than guessing.
        assert_eq!(local_utc_offset_secs(), 0);
    }
}
