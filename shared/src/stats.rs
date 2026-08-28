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

/// A library-scale total and the coverage behind it.
///
/// `total` and `books` travel as a pair: every input is nullable-or-zero when
/// unmeasured, so a total published without its denominator reports a
/// partly-backfilled library as a confidently smaller one. `books == 0` means
/// nothing has been measured at all, which the surfaces render as an empty
/// state rather than a zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MeasuredTotal {
    pub total: i64,
    /// Books that contributed to `total`. Always `<= LibrarySize::books`.
    pub books: i64,
}

impl MeasuredTotal {
    /// True when nothing in the library has been measured for this figure.
    pub fn is_empty(&self) -> bool {
        self.books == 0
    }
}

/// How big the library is in the units a reader thinks in — words, pages, and
/// hours of audio.
///
/// **Library-scoped, not user-scoped**, and deliberately not a field on
/// [`StatsSummary`]: it is the same answer for every reader and only moves on
/// a reindex, so hanging it off a per-user payload would recompute and re-send
/// it on every period switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LibrarySize {
    /// Live books — those with at least one surviving `book_files` row. The
    /// denominator every coverage figure below is read against.
    ///
    /// Ghosted books (a `books` row whose files are gone) are excluded from
    /// this *and* from every numerator: their bytes aren't on disk, so
    /// counting them would report a library larger than the one that exists
    /// and drag every coverage fraction down for rows nothing can measure.
    pub books: i64,
    /// Total words across books with a stored `word_count`.
    #[serde(default)]
    pub words: MeasuredTotal,
    /// Total pages, resolved per book through the one length ladder in
    /// `db::stats::pages` — a print-edition count, else a comic's exact
    /// image-page count, else the EPUB word estimate.
    #[serde(default)]
    pub pages: MeasuredTotal,
    /// Total seconds of audio, summed over the parts of the one file the
    /// server would actually serve for each book. A book with any unprobed
    /// part is unmeasured rather than partly counted.
    #[serde(default)]
    pub listening_seconds: MeasuredTotal,
}

impl LibrarySize {
    /// True when no figure here has been measured for a single book — the
    /// surfaces' signal to render an empty state rather than three zeroes.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.pages.is_empty() && self.listening_seconds.is_empty()
    }
}

/// One column of the time-of-day strip: an hour of the reader's **local** day
/// and the active seconds recorded in it, reading and listening together.
///
/// `hour` is 0..=23 in the reader's own clock, resolved server-side from the
/// UTC offset each session carried at capture time (`db::stats::patterns`) —
/// never from the viewing client's zone, which would make the same account
/// read differently on a phone abroad than on the desktop at home.
/// [`StatsSummary::hour_of_day`] always carries all 24, ascending, zeros
/// included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HourBucket {
    pub hour: i64,
    pub seconds: i64,
}

/// One column of the day-of-week strip: a weekday in the reader's local
/// calendar and the active seconds recorded on it.
///
/// `weekday` is 0 = Monday .. 6 = Sunday, and the server sends `label` with
/// it for the same reason `LengthBucket` carries its own: week-start is a
/// convention, and a client that assumed Sunday-first would silently draw
/// every column one place out. [`StatsSummary::day_of_week`] always carries
/// all 7, Monday first, zeros included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekdayBucket {
    pub weekday: i64,
    pub label: String,
    pub seconds: i64,
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
/// `finished_books`, `books_per_month`, `pages_read`, `pages_per_hour`,
/// `length_buckets` and `previous` — shares that one definition, live books
/// only. They must not
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
    /// Estimated reading speed over the window, in pages per hour — the
    /// context `pages_read` is missing, and the figure a reader compares
    /// against their own past rather than against anyone else's.
    ///
    /// A **seconds-weighted** mean over the books finished in the window that
    /// resolve a length *and* carry recorded reading time: a book that
    /// contributes pages contributes the hours behind them, so a book begun
    /// before the window reports a plausible rate instead of its whole length
    /// against one window's hours. Narrower than `pages_read`'s population by
    /// the books nobody has recorded reading time on, and by those whose
    /// length resolves to zero pages: a total carries a zero harmlessly, but a
    /// rate would spend that book's hours against none of its pages.
    ///
    /// Reading time only — listening is excluded, since narration speed is
    /// the narrator's, not the reader's. A book read partly in audio
    /// therefore over-reports here.
    ///
    /// `None` when no finished book in the window has both, driving the same
    /// em-dash empty state `pages_read` does.
    #[serde(default)]
    pub pages_per_hour: Option<f64>,
    /// Books finished in the window bucketed by length, plus the unknown
    /// bucket. Every bucket is present, zeros included; an all-zero set means
    /// nothing was finished, which the surfaces render as an empty state
    /// rather than flat bars.
    #[serde(default)]
    pub length_buckets: Vec<LengthBucket>,
    /// Active seconds in the window by **local** hour of day — all 24
    /// buckets, ascending, zeros included, so the shape of a day stays
    /// readable rather than collapsing to whichever hours had activity.
    /// Reading and listening together, like every other activity metric here.
    ///
    /// Bucketed server-side against the UTC offset each session recorded at
    /// capture time, so every client draws the same columns from the same
    /// payload. Sessions with no recorded offset are **not** in these totals;
    /// their seconds are reported separately as [`Self::unzoned_seconds`].
    #[serde(default)]
    pub hour_of_day: Vec<HourBucket>,
    /// Active seconds in the window by local weekday — all 7 buckets, Monday
    /// first, zeros included. Same scoping and same exclusion as
    /// [`Self::hour_of_day`], so the two strips always describe one set of
    /// sessions and sum to the same total.
    #[serde(default)]
    pub day_of_week: Vec<WeekdayBucket>,
    /// Active seconds in the window that carry no capture-time UTC offset and
    /// so can't be placed on a local clock — rows written before migration
    /// 0080, or by a client that doesn't report one.
    ///
    /// Surfaced rather than folded in: attributing them to UTC would put a
    /// reader's evening in the small hours, and attributing them to some later
    /// offset would invent a fact about where they were. A non-zero value is a
    /// disclosure the two strips are drawn over less than the window's whole
    /// total, not an error.
    #[serde(default)]
    pub unzoned_seconds: i64,
    /// The caller's goal for the **current calendar year**, `None` when none
    /// is set. Like `current_streak_days` this is deliberately *not* windowed:
    /// a goal is annual by definition, so it reads the same on every
    /// [`StatsRange`] and a period switch never moves it.
    #[serde(default)]
    pub goal: Option<ReadingGoal>,
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

    /// Whether the time-pattern strips have anything to draw.
    ///
    /// Both strips are zero-filled to a fixed width, so "no data" and "a full
    /// day of nothing" render identically — this is the predicate that tells
    /// them apart, and it is deliberately shared rather than re-derived per
    /// surface. Checks the hour strip alone: the two rollups run over one set
    /// of sessions, so a non-empty weekday strip without a non-empty hour
    /// strip is not a state the server can produce.
    pub fn has_time_patterns(&self) -> bool {
        self.hour_of_day.iter().any(|b| b.seconds > 0)
    }
}

/// The only goal kind today: distinct books finished in the calendar year.
/// The wire carries the string so a later pages/minutes goal is an added
/// value rather than a breaking shape change.
pub const GOAL_KIND_BOOKS: &str = "books";

/// Inclusive upper bound on a stored goal target. Not a judgement about how
/// much anyone can read — it exists so the progress bar's arithmetic and the
/// column's width stay bounded by something other than what a client happened
/// to POST.
pub const MAX_GOAL_TARGET: i64 = 10_000;

/// Earliest goal year the write path accepts. Bounds the column at both ends
/// so a typo'd year can't file a row no surface will ever read.
pub const MIN_GOAL_YEAR: i64 = 1900;
/// Latest goal year the write path accepts.
pub const MAX_GOAL_YEAR: i64 = 2999;

/// One reader's goal for one calendar year, paired with progress toward it.
///
/// `current` uses the **same completion definition as every other completion
/// metric on [`StatsSummary`]** — a 100% journal entry or an explicit
/// `finished` read status, on a live book, counted once per book. It is
/// bounded to the goal's own year, so raising this year's target never moves
/// last year's number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadingGoal {
    /// What is being counted — [`GOAL_KIND_BOOKS`] today.
    pub kind: String,
    /// The target the reader set. Always `>= 1`; a cleared goal is an absent
    /// [`ReadingGoal`], never a zero target.
    pub target: i64,
    /// Progress toward `target` within `year`. May exceed it — a reader past
    /// their goal is the good case, and clamping it here would hide it.
    pub current: i64,
    /// The calendar year (UTC) this goal and its `current` count belong to.
    pub year: i64,
}

impl ReadingGoal {
    /// Progress as a 0..=100 percentage, clamped for rendering. Use
    /// [`Self::current`] against [`Self::target`] for the honest ratio; this
    /// is only the bar's width.
    pub fn percent(&self) -> i64 {
        if self.target <= 0 {
            return 0;
        }
        let pct = self.current.saturating_mul(100) / self.target;
        pct.clamp(0, 100)
    }

    /// Books still to go, `0` once the goal is met or passed.
    pub fn remaining(&self) -> i64 {
        (self.target - self.current).max(0)
    }

    /// Whether the reader has reached the target.
    pub fn is_met(&self) -> bool {
        self.current >= self.target
    }
}

/// Write payload for `PUT /api/stats/goal`.
///
/// `year` and `kind` default to the server's current year and
/// [`GOAL_KIND_BOOKS`], so the common client sends `{"target": 24}` alone and
/// never bakes its own clock into the request. A `null` / absent `target`
/// **clears** the goal for that `(year, kind)` — there is no separate DELETE
/// route, because "no goal" and "a goal of zero" must not both be
/// representable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReadingGoalUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Omitted rather than sent as `null` when clearing — the two decode
    /// identically, and an omitted key keeps the wire honest about how little
    /// the usual write carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<i64>,
}

impl ReadingGoalUpdate {
    /// A set-this-year's-books-goal update — the shape every surface sends.
    pub fn books(target: i64) -> Self {
        Self {
            year: None,
            kind: None,
            target: Some(target),
        }
    }

    /// A clear-this-year's-books-goal update.
    pub fn clear_books() -> Self {
        Self {
            year: None,
            kind: None,
            target: None,
        }
    }

    /// The kind this update names, defaulting to [`GOAL_KIND_BOOKS`].
    pub fn kind_or_default(&self) -> &str {
        self.kind.as_deref().unwrap_or(GOAL_KIND_BOOKS)
    }

    /// Reject an unsupported kind, an out-of-range target, or an out-of-range
    /// year. Handlers translate `Err(_)` into 400; the db layer re-checks the
    /// same bounds as typed variants, since it is also reachable from the RPC.
    pub fn validate(&self) -> Result<(), String> {
        if self.kind_or_default() != GOAL_KIND_BOOKS {
            return Err(format!("unsupported goal kind: {}", self.kind_or_default()));
        }
        if let Some(target) = self.target {
            if !(1..=MAX_GOAL_TARGET).contains(&target) {
                return Err(format!("target must be between 1 and {MAX_GOAL_TARGET}"));
            }
        }
        if let Some(year) = self.year {
            if !(MIN_GOAL_YEAR..=MAX_GOAL_YEAR).contains(&year) {
                return Err(format!(
                    "year must be between {MIN_GOAL_YEAR} and {MAX_GOAL_YEAR}"
                ));
            }
        }
        Ok(())
    }
}
