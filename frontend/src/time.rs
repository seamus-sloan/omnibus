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

/// Local UTC offset in seconds (positive east of UTC) that was in effect at
/// `unix_secs`, read synchronously from the browser's `Date.getTimezoneOffset`
/// on web. Takes the specific timestamp — not "now" — because the offset can
/// differ across a DST transition: an older highlight or journal entry must
/// report the offset that applied on *its* date, not today's. Zero everywhere
/// else: SSR has no browser to ask, and mobile's `SystemTime` clock carries no
/// zone information.
pub fn local_utc_offset_secs(unix_secs: i64) -> i64 {
    #[cfg(feature = "web")]
    {
        let millis = (unix_secs as f64) * 1000.0;
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(millis));
        // JS convention: `getTimezoneOffset` returns (UTC - local) in
        // minutes, so local = UTC + (-offset_minutes * 60).
        (-date.get_timezone_offset() * 60.0) as i64
    }
    #[cfg(not(feature = "web"))]
    {
        let _ = unix_secs;
        0
    }
}

/// The browser's IANA zone name — `"America/Los_Angeles"` — or `None` off web
/// and wherever the runtime declines to name one.
///
/// Reported alongside [`local_utc_offset_secs`], never instead of it: the offset
/// is what the stats bucketing actually uses, and it is DST-correct for the
/// instant it was taken. The zone answers *where*, which an offset cannot —
/// `-420` is three different places depending on the month. The server stores it
/// against the session and does not resolve it today; see
/// `omnibus_shared::SessionReport::time_zone`.
///
/// Unlike the offset this takes no timestamp: `resolvedOptions` reports the
/// zone the browser is configured for, which is not a per-instant fact.
pub fn local_time_zone() -> Option<String> {
    #[cfg(feature = "web")]
    {
        // `resolved_options` hands back a plain `Object`, so the property comes
        // out through `Reflect` rather than a typed accessor.
        let zone = js_sys::Reflect::get(
            &js_sys::Intl::DateTimeFormat::default().resolved_options(),
            &wasm_bindgen::JsValue::from_str("timeZone"),
        )
        .ok()?
        .as_string()?;
        // A blank would travel as a present-but-empty value and validate would
        // reject the whole report; absent is the honest encoding for "the
        // browser didn't say".
        (!zone.trim().is_empty()).then_some(zone)
    }
    #[cfg(not(feature = "web"))]
    {
        None
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
        // must report UTC rather than guessing, for any timestamp.
        assert_eq!(local_utc_offset_secs(0), 0);
        assert_eq!(local_utc_offset_secs(1_779_019_200), 0);
    }
}
