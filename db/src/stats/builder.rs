//! The configurable chart builder: compiles an [`omnibus_shared::ChartSpec`]
//! into SQL, runs one query per measure, and aligns the answers on a shared
//! time bucket.
//!
//! **The x-axis is a shared bucket key, not a free choice.** The stats data
//! sits at four grains — sittings, the forward-progress ledger, completion
//! events, and ratings — and joining them into one wide table to pivot would
//! double-count on every multi-file, multi-genre or twice-finished book. So
//! each measure declares its grain, runs its own query, and the results are
//! zipped on the bucket key afterwards. Adding a measure is a new arm in
//! [`series_rows`], never a change to a query planner.
//!
//! Every fragment here is **reused** from the module that owns it —
//! `compute::live_finished_events`, `pages::book_pages_source`,
//! `pages::ledger_in_window`, `ratings::LIVE_RATINGS`. A builder that
//! re-derived them could quietly disagree with the curated card sitting beside
//! it, which is the same failure `composition::PUB_YEAR` exists to prevent.

use std::collections::HashMap;

use omnibus_shared::{
    ChartAggregate, ChartAxis, ChartBreakdown, ChartBucket, ChartMeasure, ChartResult, ChartSeries,
    ChartSpec, ChartSpecError, ChartUnit, BREAKDOWN_LIMIT, MAX_AXES, MAX_BUCKETS, OTHER_LABEL,
};
use sqlx::{Row, SqlitePool};

use super::compute::{live_finished_events, window_start};
use super::pages::{book_pages_source, ledger_in_window};
use super::ratings::LIVE_RATINGS;
use super::StatsError;

/// Failure space of the builder: a spec the vocabulary rejects, or the DB.
#[derive(Debug, thiserror::Error)]
pub enum ChartError {
    #[error(transparent)]
    Spec(#[from] ChartSpecError),
    #[error(transparent)]
    Stats(#[from] StatsError),
}

impl From<sqlx::Error> for ChartError {
    fn from(e: sqlx::Error) -> Self {
        ChartError::Stats(StatsError::Sqlx(e))
    }
}

/// One `(bucket, total, n)` row. `n` is the row count behind `total`, carried
/// so a breakdown's `Other` fold can re-average from sums rather than
/// averaging averages — which is wrong whenever the slices differ in size.
struct Bucketed {
    bucket: String,
    total: f64,
    n: f64,
}

/// Wrap a SQL expression yielding a UTC `YYYY-MM-DD` into this bucket's key.
///
/// **Every** bucket key in this module — the data's and the axis's alike —
/// goes through here, so a series and the axis it is plotted against cannot
/// disagree about which bucket a day belongs to. All four keys sort
/// lexicographically, which is what lets the alignment pass use a plain
/// string compare.
fn bucket_expr(bucket: ChartBucket, day: &str) -> String {
    match bucket {
        ChartBucket::Day => day.to_string(),
        // `weekday 0` advances to the coming Sunday (staying put if already
        // Sunday), so backing up six days lands on that week's Monday.
        ChartBucket::Week => format!("date({day}, 'weekday 0', '-6 days')"),
        ChartBucket::Month => format!("substr({day}, 1, 7)"),
        ChartBucket::Year => format!("substr({day}, 1, 4)"),
    }
}

/// The first UTC day a bucket key covers — the inverse of [`bucket_expr`],
/// used to seed the dense axis from the earliest bucket the data reached.
fn bucket_start_day(bucket: ChartBucket, key: &str) -> String {
    match bucket {
        // Both are already a `YYYY-MM-DD` (a week's key is its Monday).
        ChartBucket::Day | ChartBucket::Week => key.to_string(),
        ChartBucket::Month => format!("{key}-01"),
        ChartBucket::Year => format!("{key}-01-01"),
    }
}

/// The distinct `(bucket, book)` pairs a reader completed in the window.
///
/// Deduplicated on purpose: `FINISHED_EVENTS` unions the journal and the
/// read-status table, so one completion routinely produces two rows. Counting
/// those would report a book twice, and averaging over them would weight it
/// twice. Both completion measures build on this one subquery so they always
/// describe the same set of books. Binds: `user_id, user_id, start`.
fn completion_pairs(bucket: ChartBucket) -> String {
    let k = bucket_expr(bucket, "date(f.finished_at, 'unixepoch')");
    format!(
        "SELECT DISTINCT {k} AS k, f.book_uuid AS uuid \
         FROM ({}) f WHERE f.finished_at >= ?",
        live_finished_events()
    )
}

/// The `(bucket, seconds)` rows of every sitting in the window, both formats.
/// Binds: `user_id, start, user_id, start`.
fn sitting_seconds(bucket: ChartBucket) -> String {
    let k = bucket_expr(bucket, "date(started_at, 'unixepoch')");
    format!(
        "SELECT {k} AS k, seconds_read AS secs FROM reading_sessions \
             WHERE user_id = ? AND started_at >= ? \
         UNION ALL \
         SELECT {k} AS k, seconds_listened AS secs FROM listening_sessions \
             WHERE user_id = ? AND started_at >= ?"
    )
}

/// Run one measure's query and return its non-empty buckets.
///
/// The `SELECT` shape is uniform — `(k, total, n)` — so [`chart_series`] can
/// treat every measure the same once it is here. Which table each arm reads is
/// the measure's declared grain made concrete.
async fn series_rows(
    pool: &SqlitePool,
    user_id: i64,
    measure: ChartMeasure,
    bucket: ChartBucket,
    start: i64,
) -> Result<Vec<Bucketed>, ChartError> {
    let (sql, binds): (String, Vec<i64>) = match measure {
        ChartMeasure::BooksFinished => (
            format!(
                "SELECT k, CAST(COUNT(*) AS REAL) AS total, CAST(COUNT(*) AS REAL) AS n \
                 FROM ({}) GROUP BY k ORDER BY k",
                completion_pairs(bucket)
            ),
            vec![user_id, user_id, start],
        ),
        // A mean, so a book the ladder resolves to zero pages is dropped
        // rather than dragging the average toward a length nobody read —
        // the same treatment `pages::pages_per_hour` gives its denominator.
        ChartMeasure::AvgPageLength => (
            format!(
                "SELECT x.k AS k, CAST(SUM(p.pages) AS REAL) AS total, CAST(COUNT(*) AS REAL) AS n \
                 FROM ({}) x JOIN ({}) p ON p.uuid = x.uuid \
                 WHERE p.pages IS NOT NULL AND p.pages > 0 \
                 GROUP BY x.k ORDER BY x.k",
                completion_pairs(bucket),
                book_pages_source()
            ),
            vec![user_id, user_id, start],
        ),
        // Windowed on `updated_at` — when the book was rated — because that is
        // the key every other ratings aggregate uses. See `ratings::avg_stars`
        // for why, and the measure's own caveat for what it costs.
        ChartMeasure::AvgRating => (
            format!(
                "SELECT {} AS k, SUM(half_stars) / 2.0 AS total, CAST(COUNT(*) AS REAL) AS n \
                 FROM ({LIVE_RATINGS}) WHERE updated_at >= ? GROUP BY k ORDER BY k",
                bucket_expr(bucket, "date(updated_at, 'unixepoch')")
            ),
            vec![user_id, start],
        ),
        ChartMeasure::ReadingMinutes => (
            format!(
                "SELECT {} AS k, SUM(seconds_read) / 60.0 AS total, CAST(COUNT(*) AS REAL) AS n \
                 FROM reading_sessions WHERE user_id = ? AND started_at >= ? \
                 GROUP BY k ORDER BY k",
                bucket_expr(bucket, "date(started_at, 'unixepoch')")
            ),
            vec![user_id, start],
        ),
        ChartMeasure::ListeningMinutes => (
            format!(
                "SELECT {} AS k, SUM(seconds_listened) / 60.0 AS total, \
                        CAST(COUNT(*) AS REAL) AS n \
                 FROM listening_sessions WHERE user_id = ? AND started_at >= ? \
                 GROUP BY k ORDER BY k",
                bucket_expr(bucket, "date(started_at, 'unixepoch')")
            ),
            vec![user_id, start],
        ),
        ChartMeasure::SessionCount => (
            format!(
                "SELECT k, CAST(COUNT(*) AS REAL) AS total, CAST(COUNT(*) AS REAL) AS n \
                 FROM ({}) GROUP BY k ORDER BY k",
                sitting_seconds(bucket)
            ),
            vec![user_id, start, user_id, start],
        ),
        ChartMeasure::AvgSessionMinutes => (
            format!(
                "SELECT k, SUM(secs) / 60.0 AS total, CAST(COUNT(*) AS REAL) AS n \
                 FROM ({}) GROUP BY k ORDER BY k",
                sitting_seconds(bucket)
            ),
            vec![user_id, start, user_id, start],
        ),
        // The ledger's `day` is already a UTC `YYYY-MM-DD`, so it buckets
        // directly with no epoch conversion.
        ChartMeasure::PagesRead => (
            format!(
                "SELECT {} AS k, \
                        SUM(CAST(w.percent_gained AS REAL) * w.pages) / 100.0 AS total, \
                        CAST(COUNT(*) AS REAL) AS n \
                 FROM ({}) w WHERE w.pages IS NOT NULL GROUP BY k ORDER BY k",
                bucket_expr(bucket, "w.day"),
                ledger_in_window()
            ),
            vec![user_id, start],
        ),
    };

    let mut q = sqlx::query(&sql);
    for b in binds {
        q = q.bind(b);
    }
    Ok(q.fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| Bucketed {
            bucket: r.get::<String, _>("k"),
            total: r.get::<f64, _>("total"),
            n: r.get::<f64, _>("n"),
        })
        .collect())
}

/// Run a genre-split measure and return its rows keyed by slice.
///
/// Reads the reader's assigned genres out of the `metadata_overrides` JSON and
/// joins `genres` for the canonical spelling, exactly as `genre::genre_share`
/// does — grouping the raw `json_each` value instead would split "sci-fi" from
/// "Sci-Fi" into two series. Binds: `user_id, user_id, start`.
async fn breakdown_rows(
    pool: &SqlitePool,
    user_id: i64,
    measure: ChartMeasure,
    bucket: ChartBucket,
    start: i64,
) -> Result<Vec<(String, Bucketed)>, ChartError> {
    // Only the completion measures reach here (`supports_breakdown`), and they
    // differ solely in what they accumulate over the same joined set.
    let (value, extra_join, extra_where) = match measure {
        ChartMeasure::AvgPageLength => (
            "CAST(SUM(p.pages) AS REAL)",
            format!("JOIN ({}) p ON p.uuid = x.uuid", book_pages_source()),
            "AND p.pages IS NOT NULL AND p.pages > 0",
        ),
        _ => ("CAST(COUNT(*) AS REAL)", String::new(), ""),
    };
    let sql = format!(
        "SELECT g.name AS slice, x.k AS k, {value} AS total, CAST(COUNT(*) AS REAL) AS n \
         FROM ({}) x \
         JOIN metadata_overrides mo ON mo.book_uuid = x.uuid \
         JOIN json_each(mo.overrides, '$.genres') je \
         JOIN genres g ON g.name = je.value COLLATE NOCASE \
         {extra_join} \
         WHERE json_type(mo.overrides, '$.genres') IS NOT NULL {extra_where} \
         GROUP BY g.id, x.k ORDER BY x.k",
        completion_pairs(bucket)
    );
    Ok(sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("slice"),
                Bucketed {
                    bucket: r.get::<String, _>("k"),
                    total: r.get::<f64, _>("total"),
                    n: r.get::<f64, _>("n"),
                },
            )
        })
        .collect())
}

/// The dense bucket axis, ascending.
///
/// Dense on purpose: a month nobody read in is part of the story, and dropping
/// it would let the x-axis compress a gap into nothing and misreport a trend.
/// Built from a day spine in SQLite so the calendar arithmetic stays in one
/// place, then folded to bucket keys by the *same* [`bucket_expr`] the data
/// used.
///
/// It starts at the later of the window start and the earliest bucket the data
/// actually reached — which is what keeps a Lifetime range from opening the
/// axis at the unix epoch and drawing fifty empty years.
async fn axis(
    pool: &SqlitePool,
    bucket: ChartBucket,
    start: i64,
    first_data_bucket: &str,
) -> Result<Vec<String>, ChartError> {
    let sql = format!(
        "WITH RECURSIVE d(day) AS ( \
             SELECT MAX(date(?, 'unixepoch'), ?) \
             UNION ALL \
             SELECT date(day, '+1 day') FROM d WHERE day < date('now') \
         ) SELECT DISTINCT {} AS k FROM d ORDER BY k",
        bucket_expr(bucket, "day")
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(start)
        .bind(bucket_start_day(bucket, first_data_bucket))
        .fetch_all(pool)
        .await?)
}

/// Fold `(bucket, total, n)` rows into a value per bucket for this aggregate.
///
/// Summing the totals and the counts separately, then dividing once, is what
/// makes an `Average` correct across a fold — averaging the slices' own
/// averages would weight a two-book genre the same as a fifty-book one.
fn reduce(rows: &[&Bucketed], aggregate: ChartAggregate) -> Option<f64> {
    if rows.is_empty() {
        return None;
    }
    let total: f64 = rows.iter().map(|r| r.total).sum();
    match aggregate {
        ChartAggregate::Count | ChartAggregate::Sum => Some(total),
        ChartAggregate::Average => {
            let n: f64 = rows.iter().map(|r| r.n).sum();
            (n > 0.0).then(|| total / n)
        }
    }
}

/// Lay rows onto the axis, filling absent buckets per the aggregate's rule.
///
/// A `Count` or `Sum` bucket with no rows is a real zero — the reader finished
/// nothing that month. An `Average` bucket with no rows is *no data*, and
/// plotting it as zero would drag a trendline toward a number that was never
/// read.
fn align(buckets: &[String], rows: &[Bucketed], aggregate: ChartAggregate) -> Vec<Option<f64>> {
    let mut by_bucket: HashMap<&str, Vec<&Bucketed>> = HashMap::new();
    for r in rows {
        by_bucket.entry(r.bucket.as_str()).or_default().push(r);
    }
    buckets
        .iter()
        .map(|b| match by_bucket.get(b.as_str()) {
            Some(rs) => reduce(rs, aggregate),
            None => aggregate.empty_bucket(),
        })
        .collect()
}

/// Round a tick step up to a readable 1 / 2 / 2.5 / 5 × 10ⁿ value.
fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let step = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 2.5 {
        2.5
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * magnitude
}

/// How much of the axis the data must fill before the fit is accepted.
///
/// A ladder that only ever rounds the *maximum* up leaves an axis of 1000 for
/// a peak of 550 — the data occupies half the frame and every shape in it
/// reads flatter than it is. Trying several tick counts and keeping the
/// tightest honest fit is what avoids that.
const AXIS_MAX_HEADROOM: f64 = 1.35;

/// Tick counts to try, in preference order.
const AXIS_DIVISION_CHOICES: [usize; 3] = [4, 5, 3];

/// Below this peak a whole-number axis gets one gridline per unit.
const SMALL_COUNT_MAX: f64 = 6.0;

/// An axis maximum and the number of gridlines that divide it, chosen so the
/// ticks land on round numbers *and* the data fills the frame.
/// `integral` marks an axis whose values are counts — half a book is not a
/// quantity, so its gridlines must land on whole numbers.
fn nice_axis(max: f64, integral: bool) -> (f64, usize) {
    if !max.is_finite() || max <= 0.0 {
        return (1.0, AXIS_DIVISION_CHOICES[0]);
    }
    // A small count reads best with one gridline per unit, and fills the frame
    // exactly rather than rounding up to the next ladder rung.
    if integral && max <= SMALL_COUNT_MAX {
        let top = max.ceil().max(1.0);
        return (top, top as usize);
    }
    for divisions in AXIS_DIVISION_CHOICES {
        let top = axis_step(max, divisions, integral) * divisions as f64;
        if top >= max && top <= max * AXIS_MAX_HEADROOM {
            return (top, divisions);
        }
    }
    // No tick count fits inside the headroom, so take the default's ceiling
    // rather than an axis the data overflows.
    let divisions = AXIS_DIVISION_CHOICES[0];
    (
        axis_step(max, divisions, integral) * divisions as f64,
        divisions,
    )
}

/// One gridline's worth of value, rounded up to a whole number on a count axis.
fn axis_step(max: f64, divisions: usize, integral: bool) -> f64 {
    let step = nice_step(max / divisions as f64);
    if integral {
        step.ceil()
    } else {
        step
    }
}

/// An axis maximum on a *fixed* tick count.
///
/// The right-hand axis cannot pick its own count: both axes share one set of
/// gridlines, and two different counts would draw the right axis's labels
/// between the lines they belong to.
fn axis_max_on(max: f64, divisions: usize, integral: bool) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    axis_step(max, divisions, integral) * divisions as f64
}

/// Keep the `BREAKDOWN_LIMIT` most-read slices and fold the tail into one
/// `Other` series, so a library with forty genres still draws a legend
/// somebody can read. A real genre named "Other" merges into the fold rather
/// than colliding with it — two legend rows of one name is the worse outcome.
///
/// Ranked on the **book count** behind each slice, not the accumulated value:
/// for an average, the value is a length, so ranking on it would let a genre
/// holding two doorstoppers outrank one holding four novellas. For a count the
/// two are the same number, so this is the one rule both measures want.
fn top_slices(rows: &[(String, Bucketed)]) -> Vec<String> {
    let mut totals: HashMap<&str, f64> = HashMap::new();
    for (slice, b) in rows {
        *totals.entry(slice.as_str()).or_insert(0.0) += b.n;
    }
    let mut ranked: Vec<(&str, f64)> = totals.into_iter().collect();
    // Name as the tiebreak so a redraw of unchanged data keeps its colours.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked
        .into_iter()
        .take(BREAKDOWN_LIMIT)
        .map(|(name, _)| name.to_string())
        .filter(|name| name != OTHER_LABEL)
        .collect()
}

/// Compile and run a chart spec.
///
/// Deliberately uncached: the spec space is open, so there is nothing to key a
/// cache on that would hit often enough to be worth the staleness — unlike
/// `user_stats`, whose `(user_id, range)` key is closed and small.
pub async fn chart_series(
    pool: &SqlitePool,
    user_id: i64,
    spec: &ChartSpec,
) -> Result<ChartResult, ChartError> {
    spec.validate()?;
    let start = window_start(pool, spec.range).await?;
    let caveats: Vec<String> = spec.caveats().into_iter().map(str::to_string).collect();

    // Each measure runs against its own grain; the only thing they share is
    // the bucket key they are grouped by.
    let mut fetched: Vec<(ChartMeasure, Vec<Bucketed>)> = Vec::new();
    let mut split: Vec<(String, Bucketed)> = Vec::new();
    let breakdown_on = spec.breakdown == ChartBreakdown::Genre;
    if breakdown_on {
        // `validate` has already established there is exactly one measure to
        // split; `first` rather than `[0]` keeps the path panic-free anyway.
        if let Some(&m) = spec.measures.first() {
            split = breakdown_rows(pool, user_id, m, spec.bucket, start).await?;
        }
    } else {
        for &m in &spec.measures {
            fetched.push((m, series_rows(pool, user_id, m, spec.bucket, start).await?));
        }
    }

    // The earliest bucket anything landed in decides where the axis opens.
    let first = fetched
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|r| r.bucket.as_str()))
        .chain(split.iter().map(|(_, b)| b.bucket.as_str()))
        .min();
    let Some(first) = first else {
        // Nothing in the window at all: an empty axis, which the surface
        // renders as its empty state rather than as a chart of zeroes.
        return Ok(ChartResult {
            bucket: spec.bucket,
            buckets: vec![],
            series: vec![],
            axes: vec![],
            divisions: AXIS_DIVISION_CHOICES[0] as u8,
            truncated: false,
            caveats,
        });
    };

    let all = axis(pool, spec.bucket, start, first).await?;
    let truncated = all.len() > MAX_BUCKETS;
    // Keep the most recent window — a clipped axis that dropped the newest
    // buckets would answer a question nobody asked.
    let buckets: Vec<String> = if truncated {
        all[all.len() - MAX_BUCKETS..].to_vec()
    } else {
        all
    };

    let series = if breakdown_on {
        build_split_series(&buckets, &spec.measures[0], &split)
    } else {
        build_measure_series(&buckets, &fetched)
    };
    let (axes, divisions) = build_axes(&series);

    Ok(ChartResult {
        bucket: spec.bucket,
        buckets,
        series,
        axes,
        divisions: divisions as u8,
        truncated,
        caveats,
    })
}

/// One series per measure, each scaled against the axis its **unit** claims.
///
/// Units claim axes in the order their first measure was chosen, so any number
/// of measures sharing a unit sit on one scale and stay directly comparable —
/// `MAX_AXES` bounds the scales on screen, never the measures. A unit with no
/// axis left is unreachable here: `ChartSpec::validate` rejects it before any
/// query runs, and the picker greys it out before that.
fn build_measure_series(
    buckets: &[String],
    fetched: &[(ChartMeasure, Vec<Bucketed>)],
) -> Vec<ChartSeries> {
    let mut units: Vec<ChartUnit> = Vec::new();
    for (m, _) in fetched {
        if !units.contains(&m.unit()) {
            units.push(m.unit());
        }
    }
    fetched
        .iter()
        .map(|(m, rows)| ChartSeries {
            measure: *m,
            slice: None,
            axis: units.iter().position(|u| *u == m.unit()).unwrap_or(0) as u8,
            mark: m.mark(),
            values: align(buckets, rows, m.aggregate()),
        })
        .collect()
}

/// One series per surviving slice, plus the folded `Other`. Every slice shares
/// the measure's unit, so they all sit on the left axis.
fn build_split_series(
    buckets: &[String],
    measure: &ChartMeasure,
    rows: &[(String, Bucketed)],
) -> Vec<ChartSeries> {
    let kept = top_slices(rows);
    let mut series: Vec<ChartSeries> = kept
        .iter()
        .map(|name| {
            let mine: Vec<Bucketed> = rows
                .iter()
                .filter(|(s, _)| s == name)
                .map(|(_, b)| Bucketed {
                    bucket: b.bucket.clone(),
                    total: b.total,
                    n: b.n,
                })
                .collect();
            ChartSeries {
                measure: *measure,
                slice: Some(name.clone()),
                axis: 0,
                mark: measure.mark(),
                values: align(buckets, &mine, measure.aggregate()),
            }
        })
        .collect();

    let tail: Vec<Bucketed> = rows
        .iter()
        .filter(|(s, _)| !kept.contains(s))
        .map(|(_, b)| Bucketed {
            bucket: b.bucket.clone(),
            total: b.total,
            n: b.n,
        })
        .collect();
    if !tail.is_empty() {
        series.push(ChartSeries {
            measure: *measure,
            slice: Some(OTHER_LABEL.to_string()),
            axis: 0,
            mark: measure.mark(),
            values: align(buckets, &tail, measure.aggregate()),
        });
    }
    series
}

/// The one or two axes the series are scaled against, plus the gridline count
/// they share.
///
/// The left axis picks the tick count; the right is then fitted to it, because
/// one set of gridlines serves both.
fn build_axes(series: &[ChartSeries]) -> (Vec<ChartAxis>, usize) {
    let unit_of = |axis: u8| -> Option<ChartUnit> {
        series
            .iter()
            .find(|s| s.axis == axis)
            .map(|s| s.measure.unit())
    };
    let peak_of = |axis: u8| -> f64 {
        series
            .iter()
            .filter(|s| s.axis == axis)
            .filter_map(ChartSeries::max)
            .fold(0.0, f64::max)
    };

    // An axis is whole-numbered only when every series on it counts rows.
    let integral_on = |axis: u8| -> bool {
        let mut on_axis = series.iter().filter(|s| s.axis == axis).peekable();
        on_axis.peek().is_some()
            && series
                .iter()
                .filter(|s| s.axis == axis)
                .all(|s| s.measure.aggregate() == ChartAggregate::Count)
    };

    // The left axis picks the tick count; every other is fitted to it, because
    // one set of gridlines serves them all.
    let (left_max, divisions) = nice_axis(peak_of(0), integral_on(0));
    let mut axes = Vec::new();
    for a in 0..MAX_AXES as u8 {
        let Some(unit) = unit_of(a) else { continue };
        let max = if a == 0 {
            left_max
        } else {
            axis_max_on(peak_of(a), divisions, integral_on(a))
        };
        axes.push(ChartAxis { unit, max });
    }
    (axes, divisions)
}

#[cfg(test)]
mod tests;
