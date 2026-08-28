//! Reading-stats aggregate wire types.
//!
//! Produced by `db::stats` and served to the `/stats` page. One
//! [`StatsSummary`] carries the headline numbers, daily-activity heatmap,
//! top-authors/top-tags rankings, and finished-books rail for one user.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Per-user aggregate cache TTL for `db::stats::user_stats`, in seconds.
/// Lives here (rather than only in `omnibus-db`) so the `/stats` page's
/// footer freshness note can reference the real value instead of a second
/// hardcoded copy — see `db::stats::STATS_TTL_SECS`, which re-exports this.
pub const STATS_TTL_SECS: i64 = 60;

/// Reporting window for the stats page. Serializes as a compact snake-case
/// string so the wire shape stays stable across the RPC boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsRange {
    /// The rolling last 7 days (start of day, 6 days ago .. now).
    Week,
    /// The current calendar month (1st UTC .. now).
    #[default]
    Month,
    /// The current calendar year (Jan 1 UTC .. now).
    Year,
    /// Every session on record. Rendered as "Lifetime".
    AllTime,
}

impl StatsRange {
    /// Every range in switcher order.
    pub const ALL: [StatsRange; 4] = [
        StatsRange::Week,
        StatsRange::Month,
        StatsRange::Year,
        StatsRange::AllTime,
    ];

    /// Wire name matching the serde snake_case rename, for query strings.
    pub fn as_query(&self) -> &'static str {
        match self {
            StatsRange::Week => "week",
            StatsRange::Month => "month",
            StatsRange::Year => "year",
            StatsRange::AllTime => "all_time",
        }
    }

    /// Human label for the period switcher.
    pub fn label(&self) -> &'static str {
        match self {
            StatsRange::Week => "Week",
            StatsRange::Month => "Month",
            StatsRange::Year => "Year",
            StatsRange::AllTime => "Lifetime",
        }
    }
}

/// One heatmap cell: a calendar day (UTC `YYYY-MM-DD`) and the total active
/// seconds recorded that day across reading and listening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayActivity {
    pub day: String,
    pub seconds: i64,
}

/// A ranked entity (author or tag) by total active seconds in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedEntity {
    pub name: String,
    pub seconds: i64,
}

/// Reading/listening insights for one book, scoped to one user — the
/// book-detail page's Stats stop (Started / Time in book / Pickups + avg sit /
/// Longest sit, plus the per-day activity spark). Produced by
/// `db::stats::book_insights` from the same `reading_sessions` /
/// `listening_sessions` tables [`StatsSummary`] aggregates from, but scoped to
/// a single `(user, book_uuid)` rather than a user-wide window. The RPC wraps
/// this in `Option` — `None` means either that the uuid resolves to no live
/// book or that the book has no sitting worth reporting yet, both driving the
/// stop's quiet empty state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookInsights {
    /// Unix seconds of the earliest recorded session (reading or listening).
    pub started_at: i64,
    /// Total seconds across every reading and listening session on this book.
    /// Counts every recorded second, including those in sittings too short
    /// to appear in [`Self::sessions`].
    pub seconds_total: i64,
    /// Count of *sittings* — adjacent checkpoint rows across both formats
    /// stitched back together, so this reflects how often the book was picked
    /// up rather than how often the reporting client flushed. Sittings under
    /// the server's minimum are glances and don't count.
    pub sessions: i64,
    /// Seconds belonging to the sittings [`Self::sessions`] counted, glances
    /// excluded — the numerator for a per-sitting mean.
    ///
    /// Dividing [`Self::seconds_total`] by [`Self::sessions`] instead mixes
    /// two populations: a book with one real sitting and a pile of glances
    /// would report a mean above its own longest sitting. This is always
    /// `<= seconds_total`, and the mean it yields is always
    /// `<= longest_seconds`.
    #[serde(default)]
    pub sitting_seconds: i64,
    /// Seconds of the single longest sitting on this book.
    pub longest_seconds: i64,
    /// Unix seconds when that longest sitting started.
    pub longest_started_at: i64,
    /// Per-day activity on this book (active days only, ascending). Days use
    /// the same UTC `YYYY-MM-DD` bucketing as [`DayActivity`] elsewhere;
    /// callers fill calendar gaps against [`Self::as_of_day`].
    pub daily: Vec<DayActivity>,
    /// The server's current UTC day (`YYYY-MM-DD`) — the right edge of the
    /// `daily` window, so clients don't have to guess the server's "today".
    pub as_of_day: String,
}

/// One genre-donut slice: a genre and how many distinct books carrying it had
/// session activity in the window (share by book count, not seconds).
///
/// The server sends every genre, not a top-N, so the donut can size its
/// "Other" fold over the real tail instead of over a silently truncated one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenreShare {
    pub name: String,
    pub books: i64,
}

/// A book completed within the window — a 100% journal entry or an explicit
/// read-status `finished`. `finished_at` is the most recent such moment
/// (unix secs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedBook {
    pub book_uuid: String,
    pub title: String,
    pub author: Option<String>,
    pub finished_at: i64,
    /// `/api/covers/:uuid` when the book has a cover, `None` otherwise — same
    /// shape as `EbookMetadata::cover_url`, so the drill-in's finished-books
    /// list can hand it straight to `CoverTile`.
    #[serde(default)]
    pub cover_url: Option<String>,
    /// The user's star rating for this book (0.5..=5.0), `None` if unrated.
    #[serde(default)]
    pub rating: Option<f64>,
}

/// One month's finished-book count in the trailing-12-month trend chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthCount {
    /// UTC calendar month, `YYYY-MM`.
    pub month: String,
    pub books: i64,
}

/// One point in a metric's drill-in trend chart: a period label (a day
/// `YYYY-MM-DD` or a month `YYYY-MM`, depending on the series) and the
/// metric's value over that period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrendPoint {
    pub label: String,
    pub value: f64,
}

/// One bar of the star-rating distribution: a half-star bucket and how many
/// books the user rated into it within the window.
///
/// `half_stars` is the stored 1..=10 scale, **not** stars — renderers must
/// halve it, or a 5-star rating reads as a 10-point scale. `stars()` is that
/// conversion, so no surface reimplements it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatingBucket {
    pub half_stars: i64,
    pub books: i64,
}

impl RatingBucket {
    /// This bucket in stars (0.5 ..= 5.0) — the scale the UI displays.
    pub fn stars(&self) -> f64 {
        // Buckets are 1..=10 by construction, far inside f64's exact range.
        #[allow(clippy::cast_precision_loss)]
        let stars = self.half_stars as f64 / 2.0;
        stars
    }
}

/// One bar of the book-length distribution: a page-range label and how many
/// books finished in the window fall into it.
///
/// The server owns both the boundaries and their labels (`db::stats::pages`),
/// so a client never re-derives a range and disagrees about where 499 pages
/// belongs. One bucket is the **unknown** bucket — a book no rung of the
/// length ladder can measure — and it must be rendered, not dropped: an
/// audiobook has no page analogue, and silently omitting it reports a
/// distribution over fewer books than the window contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LengthBucket {
    pub label: String,
    pub books: i64,
}

/// Scalar aggregates for the **same elapsed slice** of the preceding period —
/// feeds each metric tile's drill-in delta. The current window is
/// period-to-date, so this is month-to-date against the same days last month
/// rather than against the whole of it. `Default` (all zero / `None`) for
/// [`StatsRange::AllTime`], which has no prior window to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PeriodComparison {
    pub books_finished: i64,
    pub avg_stars: Option<f64>,
    pub listening_seconds: i64,
}

/// The full aggregate payload for one user over one [`StatsRange`].
///
/// Book completion is sourced from either a 100% `journal_entries` row or an
/// explicit `book_read_status` of `finished`, never from the session tables
/// (which carry duration but no progress). A book finished both ways counts
/// once, and every completion metric on this struct — `books_finished`,
/// `finished_books`, `books_per_month`, `pages_read`, `length_buckets` and
/// `previous` — shares that one definition, live books only. They must not
/// drift apart: several of them render on the same screen.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StatsSummary {
    pub range: StatsRange,
    pub reading_seconds: i64,
    pub listening_seconds: i64,
    /// Mean of the user's star ratings (0.5..=5.0) over books rated within
    /// the window; `None` when nothing was rated. `f64` keeps the struct
    /// `PartialEq`-only.
    #[serde(default)]
    pub avg_stars: Option<f64>,
    /// Sittings in the window — checkpoint rows stitched back together per
    /// book, glances excluded. Not a row count: see [`BookInsights::sessions`].
    pub sessions: i64,
    pub active_days: i64,
    pub longest_streak_days: i64,
    /// Consecutive active days still running as of [`Self::as_of_day`] — the
    /// streak the reader is *on*, where `longest_streak_days` is the record.
    /// A run ending yesterday still counts (the day isn't over yet); one
    /// ending earlier reports zero.
    ///
    /// Server-computed so every client renders the same number rather than
    /// each deriving its own from `heatmap`. **Not windowed**, unlike every
    /// other figure here: a streak is a fact about right now, not about a
    /// reporting period, so this reads the same on every `StatsRange`. Windowed
    /// it would report 2 on the 2nd of the month for a reader 40 days deep —
    /// and the web card (fed the all-time summary) and the iOS tile (fed the
    /// period-scoped one) would disagree about the very field that exists to
    /// stop clients disagreeing.
    #[serde(default)]
    pub current_streak_days: i64,
    /// First active day (UTC `YYYY-MM-DD`) of the busiest ISO week, if any.
    pub busiest_week_start: Option<String>,
    pub busiest_week_seconds: i64,
    pub books_finished: i64,
    /// Distinct books with any session activity in the window.
    #[serde(default)]
    pub books_active: i64,
    /// Distinct books with a genre *and* activity in the window — the
    /// population `genre_share`'s slices are drawn from, and the donut's
    /// center count. Always `<= books_active`; the difference is reading the
    /// ring cannot describe, which the card discloses rather than absorbing
    /// into a total that would overstate what the slices cover.
    #[serde(default)]
    pub genre_tagged_books: i64,
    /// The server's current UTC day (`YYYY-MM-DD`) when the summary was
    /// computed. Anchors the heatmap's trailing-year grid so the client
    /// never bakes its own clock into render (rule 07).
    #[serde(default)]
    pub as_of_day: String,
    pub heatmap: Vec<DayActivity>,
    pub top_authors: Vec<RankedEntity>,
    pub top_tags: Vec<RankedEntity>,
    #[serde(default)]
    pub genre_share: Vec<GenreShare>,
    pub finished_books: Vec<FinishedBook>,
    /// Books finished per month over the trailing 12 calendar months, oldest
    /// first, ending at the current (possibly partial) month. Independent of
    /// `range` — the all-time section's trend chart is never tied to the
    /// period switcher.
    #[serde(default)]
    pub books_per_month: Vec<MonthCount>,
    /// The immediately preceding window's aggregates, for the drill-in's
    /// vs-previous-period delta.
    #[serde(default)]
    pub previous: PeriodComparison,
    /// Daily listening seconds within `range`'s window — the Listening
    /// tile's drill-in trend chart.
    #[serde(default)]
    pub listening_daily: Vec<DayActivity>,
    /// Mean star rating per month over the trailing 12 calendar months —
    /// the Avg rating tile's drill-in trend chart. Independent of `range`,
    /// same trailing-window convention as `books_per_month`.
    #[serde(default)]
    pub rating_monthly: Vec<TrendPoint>,
    /// How the window's ratings are distributed across the ten half-star
    /// buckets — the shape `avg_stars` flattens into one number. All ten
    /// buckets are present, zeros included; an empty vec means the window
    /// carries no ratings at all. Scoped exactly as `avg_stars` is, so the
    /// bucket counts sum to the set the mean is taken over.
    #[serde(default)]
    pub rating_histogram: Vec<RatingBucket>,
    /// Pages read in the window — the Pages tile. Each book finished in the
    /// window contributes its length as resolved by the one ladder in
    /// `db::stats::pages`: a print-edition page count from the metadata
    /// overrides, else a comic's exact image-page count, else the EPUB word
    /// estimate. Exact for some books and an estimate for others, which is why
    /// the tile labels itself as an estimate.
    ///
    /// `None` when no finished book in the window resolves a length on any
    /// rung — an unmeasured book contributes nothing rather than zero — which
    /// drives the tile's em-dash empty state.
    #[serde(default)]
    pub pages_read: Option<i64>,
    /// Books finished in the window bucketed by length, plus the unknown
    /// bucket. Every bucket is present, zeros included; an all-zero set means
    /// nothing was finished, which the surfaces render as an empty state
    /// rather than flat bars.
    #[serde(default)]
    pub length_buckets: Vec<LengthBucket>,
}

impl StatsSummary {
    /// Total active seconds — reading plus listening.
    pub fn total_seconds(&self) -> i64 {
        self.reading_seconds + self.listening_seconds
    }

    /// True when the user has no activity to show, driving the empty state.
    ///
    /// Tested on recorded seconds rather than [`Self::sessions`], which is a
    /// *filtered* count — sittings under the server's minimum don't reach it.
    /// A reader whose every sitting was a glance still has time read, active
    /// days, a heatmap and top authors to render, so keying the page's empty
    /// state on the count would blank a populated page.
    pub fn is_empty(&self) -> bool {
        self.total_seconds() == 0 && self.books_finished == 0
    }
}
