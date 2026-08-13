//! Small display formatters shared across surfaces that render `book_files`
//! — the hero's file picker and the admin delete dialog — [`plural`], a
//! trivial pluralizer reused by the desktop and mobile search result
//! summaries, and the [`format_date_short`]/[`format_date_month_year`] pair
//! that turns a stored date string into a human one for the landing table
//! and series pages. Pure functions, no Dioxus, so they unit-test without a
//! renderer, and — per `.claude/rules/07-hydration.md` — compute identically
//! on SSR and the WASM client since neither reads any target-specific state.

use omnibus_shared::BookFileInfo;

/// Human-readable file size (`"3.1 MB"`), or `None` when the row carries no
/// usable size — an unstat'd or zero-byte file shows no size rather than
/// `"0 B"`.
pub fn file_size(size_bytes: i64) -> Option<String> {
    let bytes = u64::try_from(size_bytes).ok().filter(|bytes| *bytes > 0)?;
    // Display-only, rendered at one decimal — precision loss above f64's
    // 2^52 exact-integer range is invisible at that granularity.
    #[allow(clippy::cast_precision_loss)]
    let (value, unit) = if bytes >= 1_000_000_000 {
        (bytes as f64 / 1_000_000_000.0, "GB")
    } else if bytes >= 1_000_000 {
        (bytes as f64 / 1_000_000.0, "MB")
    } else if bytes >= 1_000 {
        (bytes as f64 / 1_000.0, "KB")
    } else {
        return Some(format!("{bytes} B"));
    };
    Some(format!("{value:.1} {unit}"))
}

/// `"EPUB · Part 2"` — the format plus the file's own label, falling back to
/// its 1-based ordinal when it has none.
pub fn file_label(file: &BookFileInfo) -> String {
    let label = file
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Part {}", file.ordinal + 1));
    format!("{} \u{b7} {label}", file.format.to_uppercase())
}

/// Pluralizing suffix for a count: `""` for exactly one, `"s"` otherwise.
pub fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Em dash used for a date with nothing displayable — absent, unparsable, or
/// a sentinel value (see [`SENTINEL_YEAR_MAX`]).
const EM_DASH: &str = "\u{2014}";

/// Calibre's "no publish date" placeholder — `UNDEFINED_DATE =
/// datetime(101, 1, 1, tzinfo=utc)`, serialized into OPF as
/// `0101-01-01T00:00:00+00:00` — and anything at or before it in year. No
/// book in a real library has a genuine publication date that old, so any
/// parsed year in this range means "date unknown," not "ancient."
const SENTINEL_YEAR_MAX: i32 = 101;

const MONTH_NAMES: [&str; 12] = [
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

/// A calendar date pulled from a stored date string's leading `YYYY`,
/// `YYYY-MM`, or `YYYY-MM-DD` prefix.
struct ParsedDate {
    year: i32,
    month: Option<u32>,
    day: Option<u32>,
}

/// Parse the leading date out of a stored date string. Handles a full ISO
/// 8601 timestamp (`2016-05-02T21:00:00+00:00`, `2026-07-31T00:01:35Z`), the
/// SQLite `datetime()` shape `added_at` uses (`2024-01-02 03:04:05`), a bare
/// `YYYY-MM-DD`, or a year-only / year-month OPF `dc:date`. Everything from a
/// `T` or space onward (time-of-day, offset) is dropped — every caller here
/// renders a calendar date, never a clock time. Returns `None` when the
/// leading segment isn't a parseable year.
fn parse_date_prefix(raw: &str) -> Option<ParsedDate> {
    let date_part = raw.trim().split(['T', ' ']).next()?;
    let mut segments = date_part.split('-');
    let year = segments.next()?.parse().ok()?;
    let month = segments.next().and_then(|m| m.parse().ok());
    let day = segments.next().and_then(|d| d.parse().ok());
    Some(ParsedDate { year, month, day })
}

/// Full month name for a 1-based month number, or `None` when it's out of
/// range (a malformed source value) — callers fall back to a bare year.
fn month_name(month: u32) -> Option<&'static str> {
    month
        .checked_sub(1)
        .and_then(|i| MONTH_NAMES.get(i as usize))
        .copied()
}

/// Shared absent/sentinel gate for the two public formatters below: parses
/// `raw`, renders it with `render` when it names a real year, and falls back
/// to an em dash otherwise.
fn render_date(raw: &str, render: impl FnOnce(&ParsedDate) -> String) -> String {
    match parse_date_prefix(raw) {
        Some(d) if d.year > SENTINEL_YEAR_MAX => render(&d),
        _ => EM_DASH.to_string(),
    }
}

/// Format a stored date string as a short human date for a table cell —
/// `"May 2, 2016"`, falling back to `"May 2016"` or `"2016"` as the source
/// narrows. Absent, unparsable, and sentinel dates (see
/// [`SENTINEL_YEAR_MAX`]) render as an em dash.
pub fn format_date_short(raw: &str) -> String {
    render_date(raw, |d| match (d.month.and_then(month_name), d.day) {
        (Some(month), Some(day)) => format!("{month} {day}, {}", d.year),
        (Some(month), None) => format!("{month} {}", d.year),
        (None, _) => d.year.to_string(),
    })
}

/// Format a stored date string as `"May 2016"` for a series card — coarser
/// than [`format_date_short`] since a card only has room for month + year.
/// Same absent/sentinel handling.
pub fn format_date_month_year(raw: &str) -> String {
    render_date(raw, |d| match d.month.and_then(month_name) {
        Some(month) => format!("{month} {}", d.year),
        None => d.year.to_string(),
    })
}

#[cfg(test)]
mod tests;
