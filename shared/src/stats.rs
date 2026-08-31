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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DayActivity {
    pub day: String,
    pub seconds: i64,
}

/// A ranked entity (author or tag) by total active seconds in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct GenreShare {
    pub name: String,
    pub books: i64,
}

/// A book completed within the window — a 100% journal entry or an explicit
/// read-status `finished`. `finished_at` is the most recent such moment
/// (unix secs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct MonthCount {
    /// UTC calendar month, `YYYY-MM`.
    pub month: String,
    pub books: i64,
}

/// One point in a metric's drill-in trend chart: a period label (a day
/// `YYYY-MM-DD` or a month `YYYY-MM`, depending on the series) and the
/// metric's value over that period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
    /// Total seconds of audio, summed over the parts of the format the server
    /// would actually serve for each book — every volume of it, not one file.
    /// A book with any unprobed part is unmeasured rather than partly counted.
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WeekdayBucket {
    pub weekday: i64,
    pub label: String,
    pub seconds: i64,
}

/// Recorded time a book needs before it can be crowned the window's fastest
/// read, in seconds.
///
/// The metric measures *tracked* reading, and a book read mostly on another
/// device shows only its tail here — one 40-second checkpoint on the day it
/// was marked finished would otherwise take the crown from every book the
/// reader actually raced through. Lives here so the surfaces can state the
/// floor without a second copy of the number drifting from
/// `db::stats::superlatives`'s.
pub const FASTEST_READ_MIN_SECS: i64 = 1800;

/// One superlative that names a book: which book won, and the figure it won
/// with.
///
/// `value`'s **unit is the field's, not this struct's** — pages for the length
/// superlatives, seconds for the longest sit, days for the fastest read — and
/// each is documented on [`Superlatives`]. Renderers must not guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BookSuperlative {
    pub book_uuid: String,
    pub title: String,
    pub author: Option<String>,
    pub value: i64,
}

/// The window's single most-X figures — the ranked one-liners a total can't
/// say, and the shape a recap leads with.
///
/// **Every field is optional, and an absent one is the point.** A window that
/// can't support a superlative omits it rather than crowning its only datum:
/// a reader who finished one book has no "shortest", and a "longest" that is
/// also the only book is noise dressed as a finding.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Superlatives {
    /// The longest book finished in the window, in **pages** as resolved by
    /// the shared length ladder (`db::stats::pages`). Absent when nothing
    /// finished in the window resolves a length.
    #[serde(default)]
    pub longest_book: Option<BookSuperlative>,
    /// The shortest book finished in the window, in **pages**. Absent when
    /// it would name the same book as `longest_book`, or carry the same page
    /// count — a pair that reads as a range when there isn't one.
    #[serde(default)]
    pub shortest_book: Option<BookSuperlative>,
    /// The single day with the most active seconds in the window, reading and
    /// listening together. Ties break to the earliest day.
    #[serde(default)]
    pub biggest_day: Option<DayActivity>,
    /// The longest single sitting in the window, in **seconds** — stitched
    /// checkpoint rows, so this is how long the reader actually sat, not how
    /// long a client waited between flushes.
    #[serde(default)]
    pub longest_sit: Option<BookSuperlative>,
    /// The book finished in the window in the fewest **days** from its first
    /// recorded session, counting a same-day read as one day rather than
    /// zero.
    ///
    /// Only books carrying at least [`FASTEST_READ_MIN_SECS`] of recorded
    /// time are eligible, and even so this is a **lower bound** on how long
    /// the read really took — reading done before session tracking, or on a
    /// device that reports nothing, is invisible here. Surfaces must say so.
    #[serde(default)]
    pub fastest_read: Option<BookSuperlative>,
}

impl Superlatives {
    /// True when none of the five server-computed figures is present.
    ///
    /// Deliberately **not** the card's render gate. Both surfaces also draw
    /// rows off `StatsSummary` fields that live outside this struct — the
    /// busiest week and the top-ranked author and subject — so a window with
    /// only a busiest week is `is_empty()` yet still has a card to show. The
    /// gate is the assembled row list; this is a predicate over the five.
    pub fn is_empty(&self) -> bool {
        self.longest_book.is_none()
            && self.shortest_book.is_none()
            && self.biggest_day.is_none()
            && self.longest_sit.is_none()
            && self.fastest_read.is_none()
    }
}

/// What the Pages tile could and could not measure in the window, and the day
/// before which it cannot measure anything at all.
///
/// The tile's headline is a single number that has three different empty
/// states, and they mean different things: a window with no activity, a window
/// whose only activity was listening (audio has no page analogue, so zero pages
/// is the *correct* answer rather than an unknown one), and a window of real
/// reading in books whose length no rung of the ladder resolves. Collapsing all
/// three into an em-dash tells a reader who listened all week that the server
/// has no idea what they did.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PagesReadDetail {
    /// UTC `YYYY-MM-DD` the forward-progress ledger began recording. Reading
    /// before it left no position trail to difference and is unrecoverable, so
    /// a window reaching back past this date is reporting on part of itself —
    /// which the surfaces state outright rather than leaving as an unexplained
    /// discontinuity.
    #[serde(default)]
    pub since_day: Option<String>,
    /// Distinct books that contributed measured page progress in the window.
    pub measured_books: i64,
    /// Distinct books read in the window whose length no rung of the ladder
    /// resolves — real reading the total cannot include.
    pub unmeasured_books: i64,
    /// Distinct books listened to in the window. Audiobooks contribute no
    /// pages by design; this is what separates "listened, so zero pages" from
    /// "nothing happened".
    pub audio_books: i64,
    /// Pages per UTC day within the window, active days only, ascending — the
    /// tile's drill-in trend.
    #[serde(default)]
    pub daily: Vec<TrendPoint>,
    /// Whether this window opens before [`Self::since_day`], so part of it is
    /// unmeasurable no matter what the reader did.
    ///
    /// Computed server-side against the window's real start rather than
    /// inferred from the [`StatsRange`], because the range is not the fact: a
    /// Year window in the calendar year *after* the epoch is fully covered, and
    /// a Week window in the days right after it is not. Only the server knows
    /// where a period starts — that calendar math lives in SQLite — so it is
    /// the only side that can answer this without guessing.
    #[serde(default)]
    pub window_predates_ledger: bool,
}

impl PagesReadDetail {
    /// True when the window holds listening and no reading at all — the one
    /// empty state whose honest headline is `0`, not an em-dash.
    pub fn audio_only(&self) -> bool {
        self.audio_books > 0 && self.measured_books == 0 && self.unmeasured_books == 0
    }

    /// True when the window starts before the ledger did, so part of it is
    /// unmeasurable no matter what the reader did — and there is an epoch to
    /// name in the disclosure.
    pub fn predates_ledger(&self) -> bool {
        self.since_day.is_some() && self.window_predates_ledger
    }
}

/// Scalar aggregates for the **same elapsed slice** of the preceding period —
/// feeds each metric tile's drill-in delta. The current window is
/// period-to-date, so this is month-to-date against the same days last month
/// rather than against the whole of it. `Default` (all zero / `None`) for
/// [`StatsRange::AllTime`], which has no prior window to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PeriodComparison {
    pub books_finished: i64,
    pub avg_stars: Option<f64>,
    pub listening_seconds: i64,
    /// Pages read over the baseline window. Day-grained, unlike its siblings
    /// here — the ledger buckets by UTC day — so the baseline includes the whole
    /// of its boundary day, matching the current window's own partial today.
    #[serde(default)]
    pub pages_read: i64,
}

/// The full aggregate payload for one user over one [`StatsRange`].
///
/// Book completion is sourced from either a 100% `journal_entries` row or an
/// explicit `book_read_status` of `finished`, never from the session tables
/// (which carry duration but no progress). A book finished both ways counts
/// once, and every completion metric on this struct — `books_finished`,
/// `finished_books`, `books_per_month`, `pages_per_hour`, `length_buckets`
/// and `previous` — shares that one definition, live books only. They must
/// not drift apart: several of them render on the same screen.
///
/// `pages_read` is pointedly **not** one of them. It measures ground covered,
/// not books completed, and sources from the forward-progress ledger instead.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
    /// Monday (UTC `YYYY-MM-DD`) of the busiest ISO week, if any — the week's
    /// own start, so a surface can label it "Week of …" whether or not the
    /// reader happened to read on the Monday.
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
    /// Pages read in the window — the Pages tile. Sums the ground each book
    /// was actually carried over inside the window (the forward-progress
    /// ledger, migration `0083`) against its length as resolved by the one
    /// ladder in `db::stats::pages`: a print-edition page count from the
    /// metadata overrides, else a comic's exact image-page count, else the EPUB
    /// word estimate. Exact for some books and an estimate for others, which is
    /// why the surfaces keep the estimate disclosure in the drill-in.
    ///
    /// Deliberately **not** the length of the books finished in the window:
    /// that figure reported nothing for a reader who finished nothing, and
    /// dumped a whole book's length into whichever window its status flip
    /// happened to land in.
    ///
    /// `None` when no book read in the window resolves a length on any rung —
    /// an unmeasured book contributes nothing rather than zero. See
    /// [`Self::pages_detail`] before rendering that as "no data": it also
    /// covers the audio-only window, whose honest answer is zero.
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
    /// against one window's hours.
    ///
    /// Scoped to books *finished* in the window, where `pages_read` beside it
    /// is scoped to ground *covered* in it: the two answer different questions
    /// over different sets of books, so dividing one by the other — or by
    /// `reading_seconds` — is not this figure. Narrowed further by the books
    /// nobody has recorded reading time on, and by those whose length resolves
    /// to zero pages: a total carries a zero harmlessly, but a rate would spend
    /// that book's hours against none of its pages.
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
    /// The window's single most-X figures. Scoped to `range` like the rest of
    /// this struct. [`Superlatives::is_empty`] is a predicate over these five,
    /// **not** the card's render gate — the surfaces draw rows off fields
    /// outside this struct too, so gating on it would drop a card that still
    /// has something to show. See that method's docs.
    #[serde(default)]
    pub superlatives: Superlatives,
    /// The caller's goal for the **current calendar year**, `None` when none
    /// is set. Like `current_streak_days` this is deliberately *not* windowed:
    /// a goal is annual by definition, so it reads the same on every
    /// [`StatsRange`] and a period switch never moves it.
    #[serde(default)]
    pub goal: Option<ReadingGoal>,
    /// Books finished in the current calendar year, whether or not an annual
    /// goal is set.
    ///
    /// The figure a surface can show *before* a reader commits to a target —
    /// the annual counterpart of [`DailyGoals::pages_today`]. It is the same
    /// measurement [`ReadingGoal::current`] carries, over the same year bounds,
    /// computed once and shared: setting a goal must not appear to move the
    /// count it measures.
    ///
    /// Not windowed, like [`Self::goal`] itself — a calendar year is not a
    /// reporting period, so this reads the same on every [`StatsRange`].
    #[serde(default)]
    pub books_this_year: Option<i64>,
    /// The caller's standing daily goals and today's progress toward them.
    /// Not windowed either, and for the same reason as `goal`: a daily target
    /// recurs, so it reads the same whichever [`StatsRange`] the page shows.
    #[serde(default)]
    pub daily_goals: DailyGoals,
    /// What `pages_read` could and could not see in this window, plus the day
    /// the ledger behind it started. Server-owned so the web tile and the iOS
    /// tile cannot disagree about which empty state a window is in.
    #[serde(default)]
    pub pages_detail: PagesReadDetail,
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

/// The annual goal kind: distinct books finished in the calendar year. The
/// wire carries the string so a further kind is an added value rather than a
/// breaking shape change.
pub const GOAL_KIND_BOOKS: &str = "books";

/// Daily goal kind — estimated pages covered, read off the forward-progress
/// ledger. `books` has no daily analogue worth offering: a book-a-day target
/// is not what a daily goal is for, and the ledger measures the same reading
/// far more finely.
pub const GOAL_KIND_PAGES: &str = "pages";

/// Daily goal kind — minutes spent reading *and* listening, the same union
/// every other time figure on [`StatsSummary`] sums.
pub const GOAL_KIND_MINUTES: &str = "minutes";

/// Inclusive upper bound on a daily pages target. Like [`MAX_GOAL_TARGET`]
/// this is not a judgement about how much anyone can read — it exists so the
/// progress bar's arithmetic stays bounded by something other than what a
/// client happened to send.
pub const MAX_DAILY_PAGES: i64 = 2_000;

/// Inclusive upper bound on a daily minutes target — the number of minutes in
/// a day. Unlike the pages bound this one is not arbitrary: a larger target
/// could not be met by a reader who did nothing else.
pub const MAX_DAILY_MINUTES: i64 = 1_440;

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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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

/// One reader's standing daily goal for one kind, paired with today's progress
/// toward it.
///
/// Recurring rather than year-bound, so there is no `year` here: the target
/// stands until it is changed, and `day` names the day `current` was measured
/// over rather than the day the goal belongs to.
///
/// # Which day
///
/// **Kind-dependent, and deliberately so.** A minutes goal is measured over the
/// reader's *local* day, from the capture-time offset each session recorded
/// (migration `0080`) — the same treatment
/// [`StatsSummary::hour_of_day`] gets, and the only honest one for a target
/// that resets at midnight. A pages goal is measured over the **UTC** day,
/// because the forward-progress ledger buckets to a UTC `YYYY-MM-DD` and keeps
/// no timestamp to re-bucket from. So the two can name different days for the
/// same moment, by up to the reader's offset. `day` is per-goal rather than
/// shared for exactly that reason — a single day string on the parent would
/// have to be wrong for one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DailyGoal {
    /// What is being counted — [`GOAL_KIND_PAGES`] or [`GOAL_KIND_MINUTES`].
    pub kind: String,
    /// The target the reader set. Always `>= 1`; a cleared goal is an absent
    /// [`DailyGoal`], never a zero target.
    pub target: i64,
    /// Progress toward `target` on `day`. May exceed it, for the reason
    /// [`ReadingGoal::current`] may.
    pub current: i64,
    /// The day `current` was measured over, `YYYY-MM-DD`. See the type docs
    /// for which calendar that is — it is not the same for both kinds.
    pub day: String,
}

impl DailyGoal {
    /// Progress as a 0..=100 percentage, clamped for rendering. Use
    /// [`Self::current`] against [`Self::target`] for the honest ratio; this is
    /// only the bar's width.
    pub fn percent(&self) -> i64 {
        if self.target <= 0 {
            return 0;
        }
        let pct = self.current.saturating_mul(100) / self.target;
        pct.clamp(0, 100)
    }

    /// Pages or minutes still to go, `0` once the goal is met or passed.
    pub fn remaining(&self) -> i64 {
        (self.target - self.current).max(0)
    }

    /// Whether the reader has reached today's target.
    pub fn is_met(&self) -> bool {
        self.current >= self.target
    }
}

/// A reader's daily goals — at most one per kind, each independent of the
/// other and of the annual [`ReadingGoal`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DailyGoals {
    /// The pages goal, `None` when unset.
    pub pages: Option<DailyGoal>,
    /// The minutes goal, `None` when unset.
    pub minutes: Option<DailyGoal>,
    /// Pages covered today, measured whether or not a pages target is set.
    ///
    /// The figure a surface can show *before* a reader commits to a goal —
    /// what iOS renders in the ring's slot when there is no ring to draw. It is
    /// the same measurement [`DailyGoal::current`] carries, over the same UTC
    /// day, computed once and shared: the number must not change the moment a
    /// target is set, or the goal appears to move the ground it measures.
    ///
    /// `None` only when the server could not measure it at all, never as a
    /// stand-in for zero.
    #[serde(default)]
    pub pages_today: Option<i64>,
    /// Minutes read and listened today, on the reader's **local** day, whether
    /// or not a minutes target is set. The counterpart to
    /// [`Self::pages_today`], and measured exactly as the minutes goal's own
    /// `current` is — truncating, so 59 seconds is not yet a minute.
    ///
    /// Note this is the local day where `pages_today` is the UTC one, for the
    /// same reason the two goals differ: see [`crate::stats`]'s
    /// `daily_goals` docs.
    #[serde(default)]
    pub minutes_today: Option<i64>,
    /// Seconds recorded today by sessions carrying no capture-time offset,
    /// which the minutes goal therefore could not place on a local day.
    ///
    /// The same disclosure [`StatsSummary::unzoned_seconds`] makes, narrowed to
    /// the day: those seconds are real reading that neither `minutes` nor
    /// [`Self::minutes_today`] includes, and a figure that silently
    /// under-reports is worse than one that says what it could not see.
    ///
    /// Reported whether or not a minutes target is set, because
    /// `minutes_today` is shown either way — there is always something to
    /// disclose against.
    #[serde(default)]
    pub unzoned_seconds: i64,
}

impl DailyGoals {
    /// Whether the reader has any daily goal at all, driving the band's invite
    /// state.
    pub fn is_empty(&self) -> bool {
        self.pages.is_none() && self.minutes.is_none()
    }
}

/// Write payload for `PUT /api/stats/goal/daily`.
///
/// Its own route and its own type rather than a `scope` on
/// [`ReadingGoalUpdate`]: that route answers with an `Option<ReadingGoal>` that
/// the iOS client already decodes, and widening its response to carry a daily
/// goal too would break every shipped build. The two write paths stay
/// independent on the wire and meet in `db::stats`.
///
/// `kind` is required — unlike the annual update there is no single sensible
/// default, since a reader may be setting either of two goals. An absent
/// `target` **clears** that kind, on the same grounds
/// [`ReadingGoalUpdate::target`] does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DailyGoalUpdate {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<i64>,
}

impl DailyGoalUpdate {
    /// A set-this-daily-goal update.
    pub fn set(kind: &str, target: i64) -> Self {
        Self {
            kind: kind.to_string(),
            target: Some(target),
        }
    }

    /// A clear-this-daily-goal update.
    pub fn clear(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            target: None,
        }
    }

    /// The inclusive upper bound for a kind, `None` when the kind is not a
    /// daily one. The bound is per-kind because the units are: 2,000 is a
    /// generous day of pages and an impossible day of minutes.
    pub fn max_target(kind: &str) -> Option<i64> {
        match kind {
            GOAL_KIND_PAGES => Some(MAX_DAILY_PAGES),
            GOAL_KIND_MINUTES => Some(MAX_DAILY_MINUTES),
            _ => None,
        }
    }

    /// Reject an unsupported kind or an out-of-range target. Handlers
    /// translate `Err(_)` into 400; the db layer re-checks the same bounds,
    /// since it is also reachable from the RPC.
    pub fn validate(&self) -> Result<(), String> {
        let Some(max) = Self::max_target(&self.kind) else {
            return Err(format!("unsupported daily goal kind: {}", self.kind));
        };
        if let Some(target) = self.target {
            if !(1..=max).contains(&target) {
                return Err(format!("{} target must be between 1 and {max}", self.kind));
            }
        }
        Ok(())
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

/// One bucket of a [`LibraryComposition`] dimension: a display label and the
/// distinct live books behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionSlice {
    pub label: String,
    pub books: i64,
}

/// One dimension of the library's composition — its buckets plus the coverage
/// behind them.
///
/// The coverage pair is the same [`MeasuredTotal`] contract the library-size
/// figures use, read one level up: `total` is **bucket placements** (the sum
/// of every slice's `books`) and `books` is the **distinct live books** the
/// dimension describes. The two differ exactly when a book lands in more than
/// one bucket — a dual-format book, a multi-genre one — so `total - books` is
/// the overlap, and `books` against `LibraryComposition::books` is the share
/// of the library the dimension can speak for at all.
///
/// `coverage.books == 0` means nothing in the library carries this dimension,
/// which the surfaces render as an empty state rather than an empty chart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionDimension {
    #[serde(default)]
    pub slices: Vec<CompositionSlice>,
    #[serde(default)]
    pub coverage: MeasuredTotal,
}

impl CompositionDimension {
    /// True when no live book carries this dimension at all.
    pub fn is_empty(&self) -> bool {
        self.coverage.books == 0 || self.slices.is_empty()
    }

    /// Books that land in more than one bucket — the overlap a reader needs
    /// to reconcile the slices against the library total. Zero for every
    /// dimension whose buckets are mutually exclusive.
    pub fn overlap(&self) -> i64 {
        (self.coverage.total - self.coverage.books).max(0)
    }

    /// Live books this dimension has nothing to say about — no language link,
    /// no publisher, no genre override. `library_books` is
    /// [`LibraryComposition::books`].
    pub fn uncovered(&self, library_books: i64) -> i64 {
        (library_books - self.coverage.books).max(0)
    }
}

/// What the collection is made of: its format mix, language mix, publisher
/// spread, publication-decade histogram, and genre distribution.
///
/// **Library-scoped, not user-scoped**, and deliberately not a field on
/// [`StatsSummary`] for the same reason [`LibrarySize`] isn't: it is the same
/// answer for every reader and only moves on a reindex, so hanging it off a
/// per-user payload would recompute and re-send it on every period switch.
///
/// Every count is `DISTINCT` over books, never over rows. `book_files` is one
/// row per *file* (migration `0018` keys it `UNIQUE(book_id, format,
/// ordinal)`), so a twelve-part M4B is twelve rows and counting rows would
/// report one audiobook as twelve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryComposition {
    /// Live books — those with at least one surviving `book_files` row. The
    /// denominator every dimension's coverage is read against, and the same
    /// population [`LibrarySize::books`] counts.
    pub books: i64,
    /// Books whose files are gone: a `books` row with no surviving
    /// `book_files`, the rows `db::admin_health::index_status` calls ghosted.
    ///
    /// Reported rather than dropped. They carry no format at all, so they
    /// would otherwise vanish from the format rollup and leave the per-format
    /// counts quietly failing to reconcile against the library. The identity
    /// the surfaces publish is `books + ghosted_books` = every `books` row.
    #[serde(default)]
    pub ghosted_books: i64,
    /// Format mix by `book_files.format`, most books first. A book held in
    /// two formats is counted **once in each bucket** — it really is both —
    /// so the slices sum to `formats.coverage.total`, which exceeds `books`
    /// by exactly the dual-format overlap.
    #[serde(default)]
    pub formats: CompositionDimension,
    /// Language mix by `languages.code`, tail folded into "Other". Books with
    /// no language link are uncovered rather than bucketed as unknown — an
    /// absent link means the file never declared one.
    #[serde(default)]
    pub languages: CompositionDimension,
    /// Publisher spread by `publishers.name`, tail folded into "Other".
    #[serde(default)]
    pub publishers: CompositionDimension,
    /// Publication decades, oldest first, and **not folded** — a histogram
    /// sorted by height or truncated at six bars is a bar chart of nothing.
    /// The year comes from the same `CAST(substr(pubdate, 1, 4) AS INTEGER)`
    /// extraction smart-shelf rules use, so a decade histogram and a
    /// `year >= 1990` shelf can never disagree about a book.
    ///
    /// There is deliberately **no `Unknown` bucket**: a book with an absent
    /// or unparseable `pubdate` is uncovered, reported through the coverage
    /// pair like every other dimension's unknowns. That uniformity is what
    /// keeps `sum(slices) == coverage.total` checkable everywhere.
    #[serde(default)]
    pub decades: CompositionDimension,
    /// Genre distribution, tail folded into "Other".
    ///
    /// **Read its coverage before its slices.** Genres have no link table by
    /// design (migration `0066`): nothing Omnibus parses carries one, so they
    /// live only in `metadata_overrides -> '$.genres'` and this describes
    /// exactly the books someone has hand-edited. Presenting a 4%-of-library
    /// sample as "your library's genres" is the failure mode the coverage
    /// pair exists to prevent.
    #[serde(default)]
    pub genres: CompositionDimension,
}

impl LibraryComposition {
    /// True when the library has nothing to describe — no live books, or no
    /// dimension carrying a single one. The surfaces' signal to render
    /// nothing rather than five empty charts.
    pub fn is_empty(&self) -> bool {
        self.books == 0
            || (self.formats.is_empty()
                && self.languages.is_empty()
                && self.publishers.is_empty()
                && self.decades.is_empty()
                && self.genres.is_empty())
    }
}

/// Which of the two session tables a stitched sitting drew from.
///
/// [`Self::Mixed`] is not a fallback: a dual-format book read and listened to
/// in one stretch stitches into a single sitting (see `db::stats::sessionize`),
/// and reporting it as one or the other would name a format the reader didn't
/// spend the whole sitting in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SessionFormat {
    Reading,
    Listening,
    Mixed,
}

impl SessionFormat {
    /// Human label for a log row — past tense, since a logged sitting is over.
    pub fn label(&self) -> &'static str {
        match self {
            SessionFormat::Reading => "Read",
            SessionFormat::Listening => "Listened",
            SessionFormat::Mixed => "Read & listened",
        }
    }
}

/// One sitting in the reading-session log: adjacent checkpoint rows stitched
/// back together, so an entry is a sit rather than a heartbeat flush.
///
/// `seconds` is the *recorded* time across the sitting, not `ended_at -
/// started_at`: a sitting the reader paused mid-way spans more wall clock than
/// it recorded, and the recorded figure is the one every other stats surface
/// reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SessionLogEntry {
    pub book_uuid: String,
    pub title: String,
    pub format: SessionFormat,
    /// Unix seconds of the sitting's first checkpoint row.
    pub started_at: i64,
    /// Unix seconds of its last checkpoint row's end.
    pub ended_at: i64,
    /// Seconds recorded across the sitting.
    pub seconds: i64,
}

impl SessionLogEntry {
    /// This entry's cursor — what a caller passes as `before` to fetch the
    /// page that continues after it.
    pub fn cursor(&self) -> SessionCursor {
        SessionCursor {
            started_at: self.started_at,
            book_uuid: self.book_uuid.clone(),
        }
    }
}

/// A keyset cursor into the session log: the last-seen sitting's start plus
/// its book, ordered `(started_at DESC, book_uuid DESC)`.
///
/// The book uuid is not decoration. Two different books can start a sitting in
/// the same second, and a cursor carrying only `started_at` either drops the
/// tie's remainder (`<`) or repeats it forever (`<=`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SessionCursor {
    pub started_at: i64,
    pub book_uuid: String,
}

impl SessionCursor {
    /// Wire form, `"{started_at}:{book_uuid}"` — split on the **first** colon,
    /// so the uuid half is taken verbatim even if it contains one of its own.
    pub fn encode(&self) -> String {
        format!("{}:{}", self.started_at, self.book_uuid)
    }

    /// Parse [`Self::encode`]'s output. `None` on anything else — the handler
    /// turns that into a 400 rather than silently serving page one, which
    /// would loop a paging client.
    pub fn parse(raw: &str) -> Option<Self> {
        let (started, uuid) = raw.split_once(':')?;
        if uuid.is_empty() {
            return None;
        }
        Some(SessionCursor {
            started_at: started.parse().ok()?,
            book_uuid: uuid.to_string(),
        })
    }
}

/// One page of the reading-session log, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SessionLogPage {
    pub entries: Vec<SessionLogEntry>,
    /// The cursor for the next page, or `None` at the end of the log. Encoded
    /// rather than structured so a client pages by echoing it back without
    /// knowing how the keyset is built.
    #[serde(default)]
    pub next_before: Option<String>,
}
