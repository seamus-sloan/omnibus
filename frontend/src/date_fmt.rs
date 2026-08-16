//! Shared civil-date formatting for a unix-seconds timestamp. Dependency-free
//! and deterministic (no wall clock, no locale), so SSR and the first WASM
//! paint render identically. Used by `pages::settings::background_tasks`,
//! `pages::settings::health`, and `pages::book_detail::dates`.

/// Convert days since the unix epoch to a `(year, month, day)` civil date —
/// Howard Hinnant's `civil_from_days` algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
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

/// Format a unix-seconds timestamp as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn fmt_timestamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let mi = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, fmt_timestamp};

    #[test]
    fn fmt_timestamp_formats_a_known_epoch_second() {
        assert_eq!(fmt_timestamp(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }

    #[test]
    fn fmt_timestamp_formats_the_epoch_itself() {
        assert_eq!(fmt_timestamp(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn civil_from_days_round_trips_the_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
