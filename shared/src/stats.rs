//! Reading-stats aggregate wire types.
//!
//! Produced by `db::stats` and served to the `/stats` page. One
//! [`StatsSummary`] carries the headline numbers, daily-activity heatmap,
//! top-authors/top-tags rankings, and finished-books rail for one user.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

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

/// One genre-donut slice: a tag and how many distinct books carrying it had
/// session activity in the window (share by book count, not seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenreShare {
    pub name: String,
    pub books: i64,
}

/// A book completed within the window — a journal entry recorded 100% progress
/// on it. `finished_at` is the most recent such entry's timestamp (unix secs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishedBook {
    pub book_uuid: String,
    pub title: String,
    pub author: Option<String>,
    pub finished_at: i64,
}

/// One month's finished-book count in the trailing-12-month trend chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthCount {
    /// UTC calendar month, `YYYY-MM`.
    pub month: String,
    pub books: i64,
}

/// The full aggregate payload for one user over one [`StatsRange`].
///
/// Book completion is sourced from `journal_entries.progress = 100` (the only
/// progress fraction persisted today) rather than the session tables, which
/// carry duration but no progress; a first-class read/unread state (#980)
/// feeds the same field once it lands.
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
    pub sessions: i64,
    pub active_days: i64,
    pub longest_streak_days: i64,
    /// First active day (UTC `YYYY-MM-DD`) of the busiest ISO week, if any.
    pub busiest_week_start: Option<String>,
    pub busiest_week_seconds: i64,
    pub books_finished: i64,
    /// Distinct books with any session activity in the window — the genre
    /// donut's center count.
    #[serde(default)]
    pub books_active: i64,
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
}

impl StatsSummary {
    /// Total active seconds — reading plus listening.
    pub fn total_seconds(&self) -> i64 {
        self.reading_seconds + self.listening_seconds
    }

    /// True when the user has no activity to show, driving the empty state.
    pub fn is_empty(&self) -> bool {
        self.sessions == 0 && self.books_finished == 0
    }
}
