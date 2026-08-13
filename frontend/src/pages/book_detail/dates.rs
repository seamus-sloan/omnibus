//! Date formatting shared by the book-detail sections that stamp a
//! unix-seconds timestamp (journal entries, saved passages). [`fmt_long_date`]
//! itself stays dependency-free and deterministic; [`use_local_date_offset`]
//! supplies the viewer's local UTC offset reactively, starting at `0` so SSR
//! and the first client paint render the same UTC day (rule 07), then
//! reconciling to the browser's real offset in a post-mount effect on web.

use dioxus::prelude::*;

/// Viewer's local UTC offset in seconds, for [`fmt_long_date`]. Starts at `0`
/// (UTC) so SSR and the first client paint match; [`crate::time::local_utc_offset_secs`]
/// only differs from `0` on web, and only after this hook's post-mount effect
/// runs, so every reader of the returned signal re-renders once the real
/// offset lands.
#[cfg(feature = "web")]
pub(super) fn use_local_date_offset() -> ReadSignal<i64> {
    let mut offset = use_signal(|| 0i64);
    use_effect(move || {
        offset.set(crate::time::local_utc_offset_secs());
    });
    ReadSignal::new(offset)
}

/// Non-web fallback for [`use_local_date_offset`] — mobile's clock carries no
/// zone info and SSR has no browser to ask, so both stay at the `0` (UTC)
/// default the web signal also starts from.
#[cfg(not(feature = "web"))]
pub(super) fn use_local_date_offset() -> ReadSignal<i64> {
    ReadSignal::new(use_signal(|| 0i64))
}

/// Format a unix-seconds timestamp as e.g. "May 17, 2026", after shifting it
/// by `offset_secs` (pass [`use_local_date_offset`]'s value for the viewer's
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
    use super::fmt_long_date;

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
}
