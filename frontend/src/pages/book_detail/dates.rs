//! Date formatting shared by the book-detail sections that stamp a
//! unix-seconds timestamp (journal entries, saved passages). [`fmt_long_date`]
//! itself stays dependency-free and deterministic; [`use_local_dates_ready`]
//! is hoisted once per list (not once per row — a heavily-annotated book runs
//! to hundreds) and starts `false` so SSR and the first client paint render
//! the same UTC day (rule 07), flipping to `true` in a post-mount effect on
//! web. [`local_date_offset`] then resolves each row's *own* offset from its
//! *own* timestamp via [`crate::time::local_utc_offset_secs`] — never a
//! single cached "today" value shared across rows, because DST means two
//! entries on either side of a clock change carry different offsets.

use dioxus::prelude::*;

/// Whether the client is past its post-mount hydration pass, and it is
/// therefore safe to read the browser's real UTC offset. Starts `false` so
/// SSR and the first client paint match (rule 07, [`local_date_offset`]
/// returns `0` until this flips); reconciles to `true` in a post-mount effect
/// on web. Hoist this call once at the list level and thread the returned
/// signal down to each row — calling it per-row would cost one signal + one
/// effect per highlight/journal entry.
#[cfg(feature = "web")]
pub(super) fn use_local_dates_ready() -> ReadSignal<bool> {
    let mut ready = use_signal(|| false);
    use_effect(move || {
        ready.set(true);
    });
    ReadSignal::new(ready)
}

/// Non-web fallback for [`use_local_dates_ready`] — mobile's clock carries no
/// zone info and SSR has no browser to ask, so [`crate::time::local_utc_offset_secs`]
/// is always `0` there regardless of readiness; starting `true` skips the
/// pointless post-mount flip.
#[cfg(not(feature = "web"))]
pub(super) fn use_local_dates_ready() -> ReadSignal<bool> {
    ReadSignal::new(use_signal(|| true))
}

/// The offset to pass [`fmt_long_date`] for a specific `unix_secs` timestamp,
/// given whether the client is past hydration (see [`use_local_dates_ready`]).
/// `0` (UTC) before mount so the render matches SSR; otherwise this
/// timestamp's *own* historical local offset — computed fresh per call, since
/// a highlight from before a DST change and one from after it don't share an
/// offset even though both render in the same list.
pub(super) fn local_date_offset(ready: bool, unix_secs: i64) -> i64 {
    if ready {
        crate::time::local_utc_offset_secs(unix_secs)
    } else {
        0
    }
}

/// Format a unix-seconds timestamp as e.g. "May 17, 2026", after shifting it
/// by `offset_secs` (pass [`local_date_offset`]'s result for the viewer's
/// local calendar day, or `0` for the deterministic UTC day). Otherwise
/// dependency-free and side-effect-free, so it's safe in both SSR and WASM
/// renders.
pub(super) fn fmt_long_date(unix_secs: i64, offset_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let (y, m, d) = civil_from_days((unix_secs + offset_secs).div_euclid(86_400));
    let name = MONTHS
        .get((m as usize).saturating_sub(1))
        .copied()
        .unwrap_or("");
    format!("{name} {d}, {y}")
}

/// Convert days since the unix epoch to a `(year, month, day)` civil date
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    // `mp` ∈ 0..=11 and the day term ∈ 1..=31 by construction of the
    // algorithm, so both conversions are in-range.
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::{fmt_long_date, local_date_offset};

    #[test]
    fn fmt_long_date_formats_known_epoch_dates_with_no_offset() {
        assert_eq!(fmt_long_date(1_779_019_200, 0), "May 17, 2026");
        assert_eq!(fmt_long_date(1_700_000_000, 0), "November 14, 2023");
        assert_eq!(fmt_long_date(0, 0), "January 1, 1970");
    }

    #[test]
    fn fmt_long_date_handles_pre_epoch_timestamps() {
        // `div_euclid` floors, so a negative timestamp lands on the day
        // *before* the epoch rather than truncating back to it.
        assert_eq!(fmt_long_date(-1, 0), "December 31, 1969");
    }

    #[test]
    fn fmt_long_date_uses_the_viewers_local_calendar_day_not_utc() {
        // 2026-08-13T02:00:00Z — an evening write in a US-Eastern-like
        // offset (UTC-4) that lands on the *next* UTC calendar day. The
        // reported issue: a highlight/journal entry saved the evening of
        // Aug 12 local time rendered "August 13" without the offset applied.
        let unix_secs = 1_786_586_400;
        assert_eq!(fmt_long_date(unix_secs, 0), "August 13, 2026");
        assert_eq!(fmt_long_date(unix_secs, -4 * 3_600), "August 12, 2026");
    }

    #[test]
    fn fmt_long_date_rolls_forward_a_day_for_a_positive_offset() {
        // 2026-08-12T20:00:00Z, shifted by a Japan-like UTC+9 offset, lands
        // on the *next* calendar day relative to the UTC render.
        let unix_secs = 1_786_564_800;
        assert_eq!(fmt_long_date(unix_secs, 0), "August 12, 2026");
        assert_eq!(fmt_long_date(unix_secs, 9 * 3_600), "August 13, 2026");
    }

    #[test]
    fn fmt_long_date_applies_each_entrys_own_offset_across_a_dst_boundary() {
        // Two entries either side of a US-Eastern DST change (clocks fall
        // back from EDT UTC-4 to EST UTC-5 at 2025-11-02T02:00 local): each
        // must shift by *its own* historical offset, not a single value
        // shared across the list — the reason `local_date_offset` resolves
        // fresh per timestamp rather than caching one hoisted "today" value.
        let evening_before_change = 1_762_056_000; // 2025-11-02T04:00:00Z (still EDT)
        let evening_after_change = 1_762_653_600; // 2025-11-09T02:00:00Z (now EST)
        assert_eq!(
            fmt_long_date(evening_before_change, -4 * 3_600),
            "November 2, 2025"
        );
        assert_eq!(
            fmt_long_date(evening_after_change, -5 * 3_600),
            "November 8, 2025"
        );
        // Applying the *wrong* (winter) offset to the pre-change entry lands
        // on the wrong day — proof the per-timestamp offset, not a shared
        // constant, is what makes this correct.
        assert_ne!(
            fmt_long_date(evening_before_change, -5 * 3_600),
            fmt_long_date(evening_before_change, -4 * 3_600)
        );
    }

    #[test]
    fn local_date_offset_returns_zero_before_ready_regardless_of_timestamp() {
        // Pre-hydration (or SSR) render: always UTC, matching the first
        // client paint so hydration doesn't mismatch (rule 07).
        assert_eq!(local_date_offset(false, 1_779_019_200), 0);
        assert_eq!(local_date_offset(false, 0), 0);
    }

    #[test]
    fn local_date_offset_delegates_to_the_timestamps_own_offset_once_ready() {
        // Off-web (this test target), `local_utc_offset_secs` is always `0`
        // regardless of timestamp — the real per-timestamp DST-aware value
        // only differs in a browser, which this delegation is what wires up.
        assert_eq!(local_date_offset(true, 1_779_019_200), 0);
        assert_eq!(local_date_offset(true, 0), 0);
    }
}
