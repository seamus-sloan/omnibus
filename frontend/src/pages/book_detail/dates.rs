//! Date formatting shared by the book-detail sections that stamp a
//! unix-seconds timestamp (journal entries, saved passages).

/// Format a unix-seconds timestamp as e.g. "May 17, 2026". Dependency-free and
/// deterministic (no wall clock), so it's safe in both SSR and WASM renders.
pub(super) fn fmt_long_date(unix_secs: i64) -> String {
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
    let (y, m, d) = civil_from_days(unix_secs.div_euclid(86_400));
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
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::fmt_long_date;

    #[test]
    fn fmt_long_date_formats_known_epoch_dates() {
        assert_eq!(fmt_long_date(1_779_019_200), "May 17, 2026");
        assert_eq!(fmt_long_date(1_700_000_000), "November 14, 2023");
        assert_eq!(fmt_long_date(0), "January 1, 1970");
    }

    #[test]
    fn fmt_long_date_handles_pre_epoch_timestamps() {
        // `div_euclid` floors, so a negative timestamp lands on the day
        // *before* the epoch rather than truncating back to it.
        assert_eq!(fmt_long_date(-1), "December 31, 1969");
    }
}
