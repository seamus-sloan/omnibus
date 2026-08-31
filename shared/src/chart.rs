//! Wire vocabulary for the configurable chart builder: the [`ChartSpec`] a
//! reader composes, and the [`ChartResult`] the server answers it with.
//!
//! Every choice here is a closed enum. The spec is compiled to SQL in
//! `db::stats::builder`, so an open vocabulary — a free-text column or
//! aggregate name — would be a SQL-injection surface rather than a
//! convenience.

use serde::{Deserialize, Serialize};

/// How many buckets a single result may carry.
///
/// A Lifetime range at Day granularity spans a decade of points on a real
/// library, which no axis can render legibly. The result keeps the **most
/// recent** [`MAX_BUCKETS`] and sets [`ChartResult::truncated`] so the surface
/// can say so — a silently shortened axis would read as "this is all the data".
pub const MAX_BUCKETS: usize = 366;

/// The most y-scales one chart may carry.
///
/// Two, because that is how many axes a chart can label without a reader
/// having to guess which scale a mark belongs to. This bounds the *units* on
/// screen, never the number of measures: any number that share a scale can be
/// plotted together and stay directly comparable. A third distinct unit would
/// have to borrow a scale that isn't its own, which draws a mark whose height
/// means nothing.
///
/// The vocabulary bounds the rest on its own — the two largest unit groups
/// hold five measures between them, inside the palette's colour count — so
/// there is deliberately no separate cap on how many may be selected.
pub const MAX_AXES: usize = 2;

/// Width of the tail-folded breakdown: this many real series, plus `Other`.
pub const BREAKDOWN_LIMIT: usize = 5;

/// Label of the synthetic tail series, matching the composition donut's.
pub const OTHER_LABEL: &str = "Other";

/// The x-axis granularity. Always time — see the module docs on
/// `db::stats::builder` for why the x-axis is a shared bucket key rather than
/// a free choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartBucket {
    Day,
    Week,
    #[default]
    Month,
    Year,
}

impl ChartBucket {
    /// Every bucket in picker order.
    pub const ALL: [ChartBucket; 4] = [
        ChartBucket::Day,
        ChartBucket::Week,
        ChartBucket::Month,
        ChartBucket::Year,
    ];

    /// Wire name matching the serde rename, for query strings.
    pub fn as_query(&self) -> &'static str {
        match self {
            ChartBucket::Day => "day",
            ChartBucket::Week => "week",
            ChartBucket::Month => "month",
            ChartBucket::Year => "year",
        }
    }

    /// Human label for the picker.
    pub fn label(&self) -> &'static str {
        match self {
            ChartBucket::Day => "Daily",
            ChartBucket::Week => "Weekly",
            ChartBucket::Month => "Monthly",
            ChartBucket::Year => "Yearly",
        }
    }
}

/// Which table a measure is computed over.
///
/// This is the field that makes the builder tractable: two measures at
/// different grains cannot be one query without double-counting, so each
/// declares its grain and the fan-out runs them separately, aligning only on
/// the shared bucket key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartGrain {
    /// One row per sitting — `reading_sessions` / `listening_sessions`.
    Sitting,
    /// One row per (book, day) of forward progress — `reading_progress_daily`.
    Ledger,
    /// One row per completion — the `FINISHED_EVENTS` union.
    Completion,
    /// One row per rating — `user_ratings`.
    Rating,
}

impl ChartGrain {
    /// Human label, shown in the picker so a reader can see *why* two measures
    /// can't be summed together.
    pub fn label(&self) -> &'static str {
        match self {
            ChartGrain::Sitting => "per sitting",
            ChartGrain::Ledger => "per day read",
            ChartGrain::Completion => "per book finished",
            ChartGrain::Rating => "per rating",
        }
    }
}

/// How a measure aggregates, which decides both its mark and what an empty
/// bucket means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartAggregate {
    /// A count of rows. An empty bucket is a real zero.
    Count,
    /// A sum over rows. An empty bucket is a real zero.
    Sum,
    /// A mean over rows. An empty bucket is *no data*, never zero.
    Average,
}

impl ChartAggregate {
    /// What an absent bucket means for this aggregate.
    ///
    /// The whole reason the two cases are distinguished: a month in which the
    /// reader finished nothing really did have zero books, but it does not
    /// have an average page length of zero — plotting one would drag a
    /// trendline toward a number nobody read.
    pub fn empty_bucket(&self) -> Option<f64> {
        match self {
            ChartAggregate::Count | ChartAggregate::Sum => Some(0.0),
            ChartAggregate::Average => None,
        }
    }
}

/// How a series is drawn. Derived from the measure, never picked by the
/// reader — a pie chart of minutes-over-time is a question the builder should
/// not be able to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartMark {
    Bar,
    Line,
}

/// The unit a measure is expressed in. Two measures sharing a unit share an
/// axis; a second distinct unit opens the right-hand axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartUnit {
    Books,
    Pages,
    Minutes,
    Sessions,
    Stars,
}

impl ChartUnit {
    /// Axis label.
    pub fn label(&self) -> &'static str {
        match self {
            ChartUnit::Books => "books",
            ChartUnit::Pages => "pages",
            ChartUnit::Minutes => "minutes",
            ChartUnit::Sessions => "sessions",
            ChartUnit::Stars => "stars",
        }
    }
}

/// The closed measure vocabulary.
///
/// Each variant maps to a hand-written SQL fragment in `db::stats::builder`
/// that **reuses** the fragment its curated equivalent on `/stats` already
/// uses, so the two surfaces cannot report different numbers for the same
/// window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartMeasure {
    /// Distinct live books that became finished in the bucket.
    BooksFinished,
    /// Mean resolved page length of the books finished in the bucket.
    AvgPageLength,
    /// Mean star rating of books rated in the bucket.
    AvgRating,
    /// Minutes of reading recorded in the bucket.
    ReadingMinutes,
    /// Minutes of listening recorded in the bucket.
    ListeningMinutes,
    /// Sittings (reading and listening) recorded in the bucket.
    SessionCount,
    /// Mean sitting length in the bucket.
    AvgSessionMinutes,
    /// Estimated pages covered in the bucket, from the forward-progress ledger.
    PagesRead,
}

impl ChartMeasure {
    /// Every measure in picker order.
    pub const ALL: [ChartMeasure; 8] = [
        ChartMeasure::BooksFinished,
        ChartMeasure::AvgPageLength,
        ChartMeasure::AvgRating,
        ChartMeasure::ReadingMinutes,
        ChartMeasure::ListeningMinutes,
        ChartMeasure::SessionCount,
        ChartMeasure::AvgSessionMinutes,
        ChartMeasure::PagesRead,
    ];

    /// Wire name matching the serde rename, for query strings.
    pub fn as_query(&self) -> &'static str {
        match self {
            ChartMeasure::BooksFinished => "books_finished",
            ChartMeasure::AvgPageLength => "avg_page_length",
            ChartMeasure::AvgRating => "avg_rating",
            ChartMeasure::ReadingMinutes => "reading_minutes",
            ChartMeasure::ListeningMinutes => "listening_minutes",
            ChartMeasure::SessionCount => "session_count",
            ChartMeasure::AvgSessionMinutes => "avg_session_minutes",
            ChartMeasure::PagesRead => "pages_read",
        }
    }

    /// Parse a wire name back, for a spec restored from a query string.
    pub fn from_query(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_query() == raw)
    }

    /// Human label for the picker and the legend.
    pub fn label(&self) -> &'static str {
        match self {
            ChartMeasure::BooksFinished => "Books finished",
            ChartMeasure::AvgPageLength => "Avg book length",
            ChartMeasure::AvgRating => "Avg rating",
            ChartMeasure::ReadingMinutes => "Reading minutes",
            ChartMeasure::ListeningMinutes => "Listening minutes",
            ChartMeasure::SessionCount => "Sittings",
            ChartMeasure::AvgSessionMinutes => "Avg sitting length",
            ChartMeasure::PagesRead => "Pages read",
        }
    }

    /// The table this measure is computed over.
    pub fn grain(&self) -> ChartGrain {
        match self {
            ChartMeasure::BooksFinished | ChartMeasure::AvgPageLength => ChartGrain::Completion,
            ChartMeasure::AvgRating => ChartGrain::Rating,
            ChartMeasure::ReadingMinutes
            | ChartMeasure::ListeningMinutes
            | ChartMeasure::SessionCount
            | ChartMeasure::AvgSessionMinutes => ChartGrain::Sitting,
            ChartMeasure::PagesRead => ChartGrain::Ledger,
        }
    }

    /// How this measure aggregates.
    pub fn aggregate(&self) -> ChartAggregate {
        match self {
            ChartMeasure::BooksFinished | ChartMeasure::SessionCount => ChartAggregate::Count,
            ChartMeasure::ReadingMinutes
            | ChartMeasure::ListeningMinutes
            | ChartMeasure::PagesRead => ChartAggregate::Sum,
            ChartMeasure::AvgPageLength
            | ChartMeasure::AvgRating
            | ChartMeasure::AvgSessionMinutes => ChartAggregate::Average,
        }
    }

    /// The unit this measure is expressed in.
    pub fn unit(&self) -> ChartUnit {
        match self {
            ChartMeasure::BooksFinished => ChartUnit::Books,
            ChartMeasure::AvgPageLength | ChartMeasure::PagesRead => ChartUnit::Pages,
            ChartMeasure::AvgRating => ChartUnit::Stars,
            ChartMeasure::ReadingMinutes
            | ChartMeasure::ListeningMinutes
            | ChartMeasure::AvgSessionMinutes => ChartUnit::Minutes,
            ChartMeasure::SessionCount => ChartUnit::Sessions,
        }
    }

    /// The mark this measure is drawn with — totals as bars, means as a line.
    /// Derived, never chosen.
    pub fn mark(&self) -> ChartMark {
        match self.aggregate() {
            ChartAggregate::Count | ChartAggregate::Sum => ChartMark::Bar,
            ChartAggregate::Average => ChartMark::Line,
        }
    }

    /// Whether this measure supports a breakdown split.
    ///
    /// Only the completion-grain measures do: a genre is a property of a
    /// *book*, so splitting a sitting — which may cover several books, and a
    /// book which may carry several genres — would attribute the same minutes
    /// to more than one series and report a total larger than the truth.
    pub fn supports_breakdown(&self) -> bool {
        self.grain() == ChartGrain::Completion
    }

    /// A coverage caveat this measure carries into the chart, when it has one.
    ///
    /// `PagesRead` is bounded by the migration `0083` ledger epoch — reading
    /// before it left no position trail to difference and cannot be
    /// reconstructed — so a window reaching further back is genuinely partial
    /// rather than quiet.
    pub fn caveat(&self) -> Option<&'static str> {
        match self {
            ChartMeasure::PagesRead => Some(
                "Pages read is measured from the progress ledger, which only \
                 covers reading recorded after it was introduced. Earlier \
                 buckets read low.",
            ),
            ChartMeasure::AvgPageLength => Some(
                "Book length is estimated from the publisher page count, or \
                 from word count where none is recorded.",
            ),
            ChartMeasure::AvgRating => Some(
                "Ratings are bucketed by when the book was rated, not when it \
                 was finished.",
            ),
            _ => None,
        }
    }
}

/// An optional split of a single measure into several series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChartBreakdown {
    #[default]
    None,
    /// Split by the reader's assigned genres (`metadata_overrides`).
    Genre,
}

impl ChartBreakdown {
    /// Every breakdown in picker order.
    pub const ALL: [ChartBreakdown; 2] = [ChartBreakdown::None, ChartBreakdown::Genre];

    /// Human label for the picker.
    pub fn label(&self) -> &'static str {
        match self {
            ChartBreakdown::None => "No split",
            ChartBreakdown::Genre => "By genre",
        }
    }
}

/// What a reader composed in the builder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChartSpec {
    /// One or two measures. A second opens the right-hand axis when its unit
    /// differs from the first's.
    pub measures: Vec<ChartMeasure>,
    /// The shared x-axis granularity.
    pub bucket: ChartBucket,
    /// The window, reusing the stats page's own range vocabulary.
    pub range: crate::stats::StatsRange,
    /// Optional split, honoured only for a single breakdown-capable measure.
    #[serde(default)]
    pub breakdown: ChartBreakdown,
}

impl Default for ChartSpec {
    fn default() -> Self {
        Self {
            measures: vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
            bucket: ChartBucket::Month,
            range: crate::stats::StatsRange::Year,
            breakdown: ChartBreakdown::None,
        }
    }
}

/// Why a [`ChartSpec`] was rejected.
///
/// Typed rather than a bare string because the builder UI renders a per-case
/// message beside the offending control.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChartSpecError {
    #[error("pick at least one measure")]
    NoMeasures,
    #[error("{0} needs a third scale — a chart has room for two")]
    TooManyUnits(&'static str),
    #[error("{0} is already on this chart")]
    DuplicateMeasures(&'static str),
    #[error("a breakdown needs exactly one measure")]
    BreakdownNeedsOneMeasure,
    #[error("{0} cannot be split — a genre belongs to a book, not a sitting")]
    BreakdownUnsupported(&'static str),
}

impl ChartSpec {
    /// Reject a spec the builder must not compile to SQL.
    ///
    /// Called on the server before any query runs, not merely in the UI: the
    /// spec arrives over RPC, so the UI's own guards are a convenience and
    /// this is the contract.
    pub fn validate(&self) -> Result<(), ChartSpecError> {
        if self.measures.is_empty() {
            return Err(ChartSpecError::NoMeasures);
        }
        let mut seen: Vec<ChartMeasure> = Vec::new();
        let mut units: Vec<ChartUnit> = Vec::new();
        for &m in &self.measures {
            if seen.contains(&m) {
                return Err(ChartSpecError::DuplicateMeasures(m.label()));
            }
            seen.push(m);
            if !units.contains(&m.unit()) {
                if units.len() == MAX_AXES {
                    return Err(ChartSpecError::TooManyUnits(m.label()));
                }
                units.push(m.unit());
            }
        }
        if self.breakdown != ChartBreakdown::None {
            if self.measures.len() != 1 {
                return Err(ChartSpecError::BreakdownNeedsOneMeasure);
            }
            let m = self.measures[0];
            if !m.supports_breakdown() {
                return Err(ChartSpecError::BreakdownUnsupported(m.label()));
            }
        }
        Ok(())
    }

    /// The distinct units on this chart, in the order their first measure was
    /// chosen — which is also the order they claim axes in.
    pub fn units(&self) -> Vec<ChartUnit> {
        let mut units: Vec<ChartUnit> = Vec::new();
        for m in &self.measures {
            if !units.contains(&m.unit()) {
                units.push(m.unit());
            }
        }
        units
    }

    /// Which axis `measure` would be drawn against, or `None` when both are
    /// already claimed by other units.
    ///
    /// This is the compatibility rule the picker greys options out by, and the
    /// reason it is here rather than in the UI: the server assigns axes from
    /// the same function, so a control that looked available can never produce
    /// a spec the server then rejects.
    pub fn axis_for(&self, measure: ChartMeasure) -> Option<u8> {
        let units = self.units();
        match units.iter().position(|u| *u == measure.unit()) {
            Some(i) => Some(i as u8),
            None if units.len() < MAX_AXES => Some(units.len() as u8),
            None => None,
        }
    }

    /// Whether `measure` can join this chart: not already on it, and its unit
    /// either already has an axis or can claim the free one.
    pub fn can_add(&self, measure: ChartMeasure) -> bool {
        !self.measures.contains(&measure) && self.axis_for(measure).is_some()
    }

    /// Add or remove `measure`, keeping selection order — the first measure
    /// chosen owns the left axis, so a toggle must not quietly reorder it.
    ///
    /// Adding an incompatible measure is a no-op rather than an error: the
    /// control offering it is already disabled, and a toggle that threw would
    /// only duplicate that guard.
    pub fn toggle(&mut self, measure: ChartMeasure) {
        if let Some(i) = self.measures.iter().position(|m| *m == measure) {
            self.measures.remove(i);
        } else if self.can_add(measure) {
            self.measures.push(measure);
        }
    }

    /// Every caveat the selected measures carry, deduplicated in measure order.
    pub fn caveats(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for m in &self.measures {
            if let Some(c) = m.caveat() {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        out
    }
}

/// One plotted series: a measure (optionally narrowed to a breakdown slice)
/// and its value in every bucket of the shared axis.
///
/// `values` is positionally aligned with [`ChartResult::buckets`] and always
/// the same length. `None` means *no data*, which for an average is a real
/// state and not a zero — see [`ChartAggregate::empty_bucket`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChartSeries {
    pub measure: ChartMeasure,
    /// The breakdown slice this series describes, when the spec asked for one.
    pub slice: Option<String>,
    /// Which axis this series is scaled against — `0` left, `1` right.
    pub axis: u8,
    pub mark: ChartMark,
    pub values: Vec<Option<f64>>,
}

impl ChartSeries {
    /// The legend entry: the measure, qualified by its slice when split.
    pub fn label(&self) -> String {
        match &self.slice {
            Some(s) => format!("{} · {s}", self.measure.label()),
            None => self.measure.label().to_string(),
        }
    }

    /// The largest value in the series, ignoring absent buckets. `None` when
    /// the series carries no data at all.
    pub fn max(&self) -> Option<f64> {
        self.values
            .iter()
            .flatten()
            .copied()
            .fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a| a.max(v)))
            })
    }
}

/// One axis of the rendered chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChartAxis {
    pub unit: ChartUnit,
    /// Upper bound of the axis. Always > 0 — a flat-zero axis is nudged to 1
    /// so the renderer never divides by zero.
    pub max: f64,
}

/// The answer to a [`ChartSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChartResult {
    /// The granularity `buckets` are keyed at. Carried on the answer so a
    /// renderer can label the axis without being handed the spec separately —
    /// and so a result can never be labelled at a granularity it wasn't
    /// bucketed at.
    pub bucket: ChartBucket,
    /// The shared x-axis: bucket keys ascending. `YYYY-MM-DD` for Day and
    /// Week (the week's Monday), `YYYY-MM` for Month, `YYYY` for Year.
    pub buckets: Vec<String>,
    pub series: Vec<ChartSeries>,
    /// Left axis, then optionally the right.
    pub axes: Vec<ChartAxis>,
    /// Gridlines dividing the plot, shared by both axes. Two different counts
    /// would put the right axis's labels between the lines they belong to.
    pub divisions: u8,
    /// Whether the bar series stack into one column per bucket.
    ///
    /// Only ever true for a **breakdown of one additive measure**, where the
    /// slices are parts of a whole and their sum is the figure the unsplit
    /// chart would show. Two different measures never stack — books on top of
    /// pages is not a quantity — and neither does a split average, since means
    /// do not add.
    pub stacked: bool,
    /// Set when the axis was clipped to the most recent [`MAX_BUCKETS`].
    pub truncated: bool,
    /// Caveats carried by the selected measures.
    pub caveats: Vec<String>,
}

impl ChartResult {
    /// Whether there is anything to draw — no buckets at all, or every series
    /// empty in every bucket.
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
            || self
                .series
                .iter()
                .all(|s| s.values.iter().all(|v| v.is_none()))
    }
}

#[cfg(test)]
mod tests;
