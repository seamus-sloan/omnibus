//! Small display formatters shared across surfaces: file size/label for
//! `book_files` pickers, [`plural`] / [`plural_noun`] / [`count_label`] for
//! counted nouns,
//! [`facet_query`] for taxonomy refinements, and the
//! [`format_date_short`]/[`format_date_month_year`]/[`format_year`] trio that
//! every reader-facing date goes through. Pure functions with no
//! Dioxus/renderer state, so they unit-test easily and render identically on
//! SSR and WASM (rule 07).

use omnibus_shared::BookFileInfo;

/// Build a `<prefix>:`-scoped FTS query from a taxonomy name, so clicking a
/// tag or genre narrows the search to the books carrying it.
///
/// **Every whitespace word is scoped individually** — `tag:Dark tag:academia`,
/// not `tag:Dark academia`. `db::helpers::build_fts_match` routes one token
/// per `prefix:` marker and lets everything else fall through to free text,
/// so a bare `format!("tag:{name}")` silently turns every word after the
/// first into an unscoped term: it would match books merely *titled*
/// something with "academia" in them, and drop the scoping the click
/// promised.
///
/// One definition for the palette rows, the `/search` chips, and the mobile
/// rows — three surfaces that each had their own copy and did not agree.
pub fn facet_query(prefix: &str, value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| format!("{prefix}:{word}"))
        .collect::<Vec<_>>()
        .join(" ")
}

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

/// The noun alone, pluralized on `n`: `"day"` at exactly one, `"days"`
/// otherwise. For surfaces that render the figure and its unit in separate
/// elements (the streak cards), where [`count_label`] can't be used.
///
/// Regular `-s` plurals only — an irregular noun needs its own two-arm
/// match at the call site.
pub fn plural_noun(n: i64, singular: &str) -> String {
    if n == 1 {
        singular.to_string()
    } else {
        format!("{singular}s")
    }
}

/// `"1 session"` / `"4 sessions"` — a count and its noun in one string,
/// pluralized on the count. Reach for this instead of `format!("{n} nouns")`:
/// a hand-written plural noun renders "1 days in" the moment the count is one
/// (#2250).
pub fn count_label(n: i64, singular: &str) -> String {
    format!("{n} {}", plural_noun(n, singular))
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

/// Abbreviated month names — the reader-facing date reads `Aug 1st, 2026`,
/// short enough for a table cell and unambiguous in every locale that reads
/// the app.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
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
///
/// A day outside 1–31 (`YYYY-MM-00`, a stray admin override, …) is treated
/// the same as an absent day rather than parsed literally — narrowing to
/// month/year is the graceful failure, not `"Month 0, YYYY"`.
fn parse_date_prefix(raw: &str) -> Option<ParsedDate> {
    let date_part = raw.trim().split(['T', ' ']).next()?;
    let mut segments = date_part.split('-');
    let year = segments.next()?.parse().ok()?;
    let month = segments.next().and_then(|m| m.parse().ok());
    let day = segments
        .next()
        .and_then(|d| d.parse().ok())
        .filter(|d| (1..=31).contains(d));
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

/// Shared absent/sentinel gate for the public formatters below: parses `raw`
/// and renders it with `render`, or `None` when it names no real year.
fn render_date(raw: &str, render: impl FnOnce(&ParsedDate) -> String) -> Option<String> {
    parse_date_prefix(raw)
        .filter(|d| d.year > SENTINEL_YEAR_MAX)
        .map(|d| render(&d))
}

/// English ordinal suffix for a day of the month — `1st`, `2nd`, `3rd`,
/// `4th`, with the 11th/12th/13th exceptions the naive last-digit rule gets
/// wrong.
fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 100, day % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    }
}

/// Format a stored date string as the reader-facing date every surface uses —
/// `"Aug 1st, 2026"`, falling back to `"Aug 2026"` or `"2026"` as the source
/// narrows. Absent, unparsable, and sentinel dates (see [`SENTINEL_YEAR_MAX`])
/// render as an em dash.
pub fn format_date_short(raw: &str) -> String {
    format_date_short_opt(raw).unwrap_or_else(|| EM_DASH.to_string())
}

/// [`format_date_short`]'s answer, or `None` where that would render an em
/// dash — for a surface (the book-detail metadata table) that drops an absent
/// date's row entirely rather than printing a dash beside rows of facts.
pub fn format_date_short_opt(raw: &str) -> Option<String> {
    render_date(raw, |d| match (d.month.and_then(month_name), d.day) {
        (Some(month), Some(day)) => {
            format!("{month} {day}{}, {}", ordinal_suffix(day), d.year)
        }
        (Some(month), None) => format!("{month} {}", d.year),
        (None, _) => d.year.to_string(),
    })
}

/// The bare publication year for a kicker or a cover caption, or `None` when
/// the source names no real year. Goes through the same parse and sentinel
/// gate as [`format_date_short`], so a kicker can never disagree with the
/// table cell beside it — a naive `raw.get(0..4)` renders Calibre's
/// `0101-01-01` placeholder as the year `0101`.
pub fn format_year(raw: &str) -> Option<String> {
    render_date(raw, |d| d.year.to_string())
}

/// Format a stored date string as `"May 2016"` for a series card — coarser
/// than [`format_date_short`] since a card only has room for month + year,
/// on the same abbreviated month names. Same absent/sentinel handling.
pub fn format_date_month_year(raw: &str) -> String {
    render_date(raw, |d| match d.month.and_then(month_name) {
        Some(month) => format!("{month} {}", d.year),
        None => d.year.to_string(),
    })
    .unwrap_or_else(|| EM_DASH.to_string())
}

#[cfg(test)]
mod tests;
