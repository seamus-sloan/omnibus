//! Reading goals: the stored `reading_goals` targets for one reader, paired
//! with progress measured the way the rest of `db::stats` measures it. An
//! annual goal counts books finished in a calendar year; a daily goal counts
//! pages or minutes today. Read by `compute`, written by the `/api/stats/goal`
//! handlers and the web RPC through [`set_goal`] and [`set_daily_goal`].

use omnibus_shared::{
    DailyGoal, DailyGoalUpdate, DailyGoals, ReadingGoal, ReadingGoalUpdate, GOAL_KIND_BOOKS,
    GOAL_KIND_MINUTES, GOAL_KIND_PAGES, MAX_GOAL_TARGET, MAX_GOAL_YEAR, MIN_GOAL_YEAR,
};
use sqlx::{Row, SqlitePool};

use crate::user_offset;

use super::{calendar, compute, pages, patterns::SESSION_LOCAL_ROWS, StatsError};

/// How far back the local-day minutes scan reaches before filtering on the
/// reader's day, in seconds.
///
/// A session's local day is its UTC timestamp shifted by up to ±14 hours, so
/// every row that can land on today sits inside a 48-hour tail — comfortably,
/// and with room for a device whose clock runs ahead. The bound exists so the
/// scan can use the `(user_id, started_at)` index instead of reading a
/// reader's whole session history to find one day's worth.
const LOCAL_DAY_SCAN_SECS: i64 = 172_800;

/// Failure space of the goal write paths. Every variant but `Sqlx` is a
/// boundary check the handler renders as a 400, so callers branch on them.
#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("unsupported goal kind: {0}")]
    UnsupportedKind(String),
    #[error("target must be between 1 and {MAX_GOAL_TARGET}: got {0}")]
    InvalidTarget(i64),
    /// Separate from [`GoalError::InvalidTarget`] because the bound is
    /// per-kind: 2,000 is a generous day of pages and an impossible day of
    /// minutes, so one shared message could only name the wrong number for
    /// one of them.
    #[error("{kind} target must be between 1 and {max}: got {got}")]
    InvalidDailyTarget { kind: String, max: i64, got: i64 },
    #[error("year must be between {MIN_GOAL_YEAR} and {MAX_GOAL_YEAR}: got {0}")]
    InvalidYear(i64),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<StatsError> for GoalError {
    fn from(e: StatsError) -> Self {
        match e {
            StatsError::Sqlx(inner) => Self::Sqlx(inner),
        }
    }
}

impl From<user_offset::OffsetError> for GoalError {
    fn from(e: user_offset::OffsetError) -> Self {
        match e {
            user_offset::OffsetError::Sqlx(inner) => Self::Sqlx(inner),
        }
    }
}

/// The current calendar year on the reader's own clock. Read from SQLite rather
/// than the process clock so it lands on the same "now" every other stats window
/// does — and on their calendar, so a reader east of UTC filing a goal at 08:00
/// on January 1st files it against the year they are actually in.
pub async fn current_year(pool: &SqlitePool, offset_minutes: i64) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT CAST(strftime('%Y', datetime('now', '{offset_minutes} minutes')) AS INTEGER)"
    );
    Ok(sqlx::query_scalar(&sql).fetch_one(pool).await?)
}

/// Unix-second bounds `[start, end)` of `year` on the reader's calendar — the
/// UTC instants their January 1sts actually fall on, so the count matches the
/// year the rest of the page is reporting.
async fn year_bounds(
    pool: &SqlitePool,
    year: i64,
    offset_minutes: i64,
) -> Result<(i64, i64), StatsError> {
    // Both bounds shift by the same offset, so the pair stays exactly one year
    // wide; `strftime` reads the literal as UTC and the subtraction moves it to
    // the instant that wall-clock moment really was.
    let shift = offset_minutes * 60;
    let row = sqlx::query(
        "SELECT CAST(strftime('%s', ? || '-01-01 00:00:00') AS INTEGER) - ? AS lo,
                CAST(strftime('%s', ? || '-01-01 00:00:00') AS INTEGER) - ? AS hi",
    )
    .bind(format!("{year:04}"))
    .bind(shift)
    .bind(format!("{:04}", year + 1))
    .bind(shift)
    .fetch_one(pool)
    .await?;
    Ok((row.try_get("lo")?, row.try_get("hi")?))
}

/// The caller's goal for the current calendar year, and that year's finished
/// count whether or not a goal exists — the two values `compute` hangs off
/// [`omnibus_shared::StatsSummary::goal`] and `books_this_year`.
///
/// The count runs unconditionally so a surface can report the year before a
/// reader commits to a target, the same reason `daily_goals` measures both
/// kinds. It is computed once and handed to both, so a goal's `current` and
/// the bare figure can never disagree.
///
/// The cost is one extra bounded count for a reader with no annual goal, on an
/// uncached summary only.
pub(super) async fn current_goal_and_progress(
    pool: &SqlitePool,
    user_id: i64,
    offset_minutes: i64,
) -> Result<(Option<ReadingGoal>, i64), StatsError> {
    let year = current_year(pool, offset_minutes).await?;
    let current = year_count(pool, user_id, year, offset_minutes).await?;
    let goal = year_target(pool, user_id, year)
        .await?
        .map(|target| ReadingGoal {
            kind: GOAL_KIND_BOOKS.to_string(),
            target,
            current,
            year,
        });
    Ok((goal, current))
}

/// The caller's stored target for `year`, `None` when they have set none.
///
/// Split out so [`current_goal_and_progress`] and [`goal_for_year`] read the
/// row the same way — a second copy of this filter is a second place for the
/// goal-kind or scope predicate to drift.
async fn year_target(
    pool: &SqlitePool,
    user_id: i64,
    year: i64,
) -> Result<Option<i64>, StatsError> {
    Ok(sqlx::query_scalar(
        "SELECT target FROM reading_goals
         WHERE user_id = ? AND scope = 'year' AND year = ? AND kind = ?",
    )
    .bind(user_id)
    .bind(year)
    .bind(GOAL_KIND_BOOKS)
    .fetch_optional(pool)
    .await?)
}

/// Books completed inside `year`, bounded to `[Jan 1 year, Jan 1 year+1)`.
///
/// Shared by both callers for the same reason [`year_target`] is: the count a
/// goal reports and the count a bare figure reports have to be one number, and
/// two copies of these bounds is how they stop being.
async fn year_count(
    pool: &SqlitePool,
    user_id: i64,
    year: i64,
    offset_minutes: i64,
) -> Result<i64, StatsError> {
    let (start, end) = year_bounds(pool, year, offset_minutes).await?;
    compute::finished_count_bounded(pool, user_id, start, end).await
}

/// The caller's goal for `year`, paired with that year's completion count.
///
/// The count is bounded to `[Jan 1 year, Jan 1 year+1)` via
/// [`compute::finished_count_bounded`], the same helper the drill-in's
/// vs-previous delta uses — so a goal filed against a past year reports that
/// year's real total and setting it changes no count anywhere.
pub async fn goal_for_year(
    pool: &SqlitePool,
    user_id: i64,
    year: i64,
    claimed_offset_minutes: Option<i64>,
) -> Result<Option<ReadingGoal>, StatsError> {
    let Some(target) = year_target(pool, user_id, year).await? else {
        return Ok(None);
    };
    let offset = user_offset::resolve_offset_minutes(pool, user_id, claimed_offset_minutes).await?;
    Ok(Some(ReadingGoal {
        kind: GOAL_KIND_BOOKS.to_string(),
        target,
        current: year_count(pool, user_id, year, offset).await?,
        year,
    }))
}

/// Set, change, or clear the caller's annual goal.
///
/// An absent `target` clears the row — there is no separate delete path, so
/// "no goal" stays a missing row rather than a zero target the ring would
/// divide by. `year` defaults to the server's current year so no client bakes
/// its own clock into the write.
///
/// Invalidates this user's cached summaries before returning: the aggregate
/// cache is keyed `(user_id, range)` with a 60s TTL, and a just-saved goal
/// reading stale is worse than a stale count.
pub async fn set_goal(
    pool: &SqlitePool,
    user_id: i64,
    update: &ReadingGoalUpdate,
    claimed_offset_minutes: Option<i64>,
) -> Result<Option<ReadingGoal>, GoalError> {
    let kind = update.kind_or_default();
    if kind != GOAL_KIND_BOOKS {
        return Err(GoalError::UnsupportedKind(kind.to_string()));
    }
    let offset = user_offset::resolve_offset_minutes(pool, user_id, claimed_offset_minutes).await?;
    let year = match update.year {
        Some(year) => year,
        None => current_year(pool, offset).await?,
    };
    if !(MIN_GOAL_YEAR..=MAX_GOAL_YEAR).contains(&year) {
        return Err(GoalError::InvalidYear(year));
    }

    match update.target {
        Some(target) => {
            if !(1..=MAX_GOAL_TARGET).contains(&target) {
                return Err(GoalError::InvalidTarget(target));
            }
            // The conflict target repeats the partial index's `WHERE`, which
            // SQLite requires to match a partial unique index at all.
            sqlx::query(
                "INSERT INTO reading_goals (user_id, scope, year, kind, target, updated_at)
                 VALUES (?, 'year', ?, ?, ?, strftime('%s','now'))
                 ON CONFLICT(user_id, year, kind) WHERE scope = 'year' DO UPDATE SET
                     target = excluded.target,
                     updated_at = strftime('%s','now')",
            )
            .bind(user_id)
            .bind(year)
            .bind(kind)
            .bind(target)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "DELETE FROM reading_goals
                 WHERE user_id = ? AND scope = 'year' AND year = ? AND kind = ?",
            )
            .bind(user_id)
            .bind(year)
            .bind(kind)
            .execute(pool)
            .await?;
        }
    }

    super::invalidate_user(user_id);
    Ok(goal_for_year(pool, user_id, year, claimed_offset_minutes).await?)
}

/// Set, change, or clear one of the caller's daily goals, returning **both**
/// afterwards.
///
/// Returning the pair rather than the one kind written is what lets a client
/// render the band from one response: the two goals are independent, so a save
/// that answered with only its own kind would leave the other's progress to a
/// second round trip that could observe a different day.
///
/// An absent `target` clears that kind, on the same grounds [`set_goal`]'s
/// does. The other kind is never touched — a reader changing their pages goal
/// has said nothing about their minutes goal.
pub async fn set_daily_goal(
    pool: &SqlitePool,
    user_id: i64,
    update: &DailyGoalUpdate,
    claimed_offset_minutes: Option<i64>,
) -> Result<DailyGoals, GoalError> {
    let Some(max) = DailyGoalUpdate::max_target(&update.kind) else {
        return Err(GoalError::UnsupportedKind(update.kind.clone()));
    };

    match update.target {
        Some(target) => {
            if !(1..=max).contains(&target) {
                return Err(GoalError::InvalidDailyTarget {
                    kind: update.kind.clone(),
                    max,
                    got: target,
                });
            }
            sqlx::query(
                "INSERT INTO reading_goals (user_id, scope, year, kind, target, updated_at)
                 VALUES (?, 'day', NULL, ?, ?, strftime('%s','now'))
                 ON CONFLICT(user_id, kind) WHERE scope = 'day' DO UPDATE SET
                     target = excluded.target,
                     updated_at = strftime('%s','now')",
            )
            .bind(user_id)
            .bind(&update.kind)
            .bind(target)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "DELETE FROM reading_goals WHERE user_id = ? AND scope = 'day' AND kind = ?",
            )
            .bind(user_id)
            .bind(&update.kind)
            .execute(pool)
            .await?;
        }
    }

    super::invalidate_user(user_id);
    Ok(daily_goals(pool, user_id, claimed_offset_minutes).await?)
}

/// The caller's standing daily goals, and today's figure for each kind
/// whether or not it carries a target.
///
/// Both measurements run unconditionally so a surface can show what a reader
/// has done today *before* they commit to a goal — the figure iOS renders in
/// the ring's slot when there is no ring to draw. The alternative, deriving it
/// client-side, was rejected: today's pages happen to be recoverable from
/// `pages_detail.daily`, but today's minutes are only recoverable from the
/// heatmap's UTC buckets, which include unzoned seconds and so would disagree
/// with what the minutes goal reports the moment one is set.
///
/// The cost is two day-scoped queries on every uncached summary, including for
/// readers with no goals at all. They are the same two the goal path already
/// ran, hoisted rather than added, and the summary sits behind the 60-second
/// aggregate cache.
///
/// A kind's [`DailyGoal::current`] and its `*_today` figure are the same
/// number by construction — computed once and shared — so setting a target
/// never appears to move the ground it measures.
///
/// # Which day both kinds are measured over
///
/// The **same** one: today on the reader's calendar, from
/// `claimed_offset_minutes`. That is what migration `0095` bought — the ledger
/// stopped writing a UTC day and started recording a quarter-hour, so pages can
/// be re-bucketed to the reader's day exactly as minutes always could. Before
/// it the two kinds answered on different calendars and could name different
/// days for the same moment.
pub async fn daily_goals(
    pool: &SqlitePool,
    user_id: i64,
    claimed_offset_minutes: Option<i64>,
) -> Result<DailyGoals, StatsError> {
    let offset = user_offset::resolve_offset_minutes(pool, user_id, claimed_offset_minutes).await?;
    daily_goals_at(pool, user_id, offset).await
}

/// [`daily_goals`] on an already-resolved offset — what `compute` calls, so a
/// summary resolves the reader's calendar once and every figure in it shares
/// that one answer.
pub(super) async fn daily_goals_at(
    pool: &SqlitePool,
    user_id: i64,
    offset_minutes: i64,
) -> Result<DailyGoals, StatsError> {
    let rows =
        sqlx::query("SELECT kind, target FROM reading_goals WHERE user_id = ? AND scope = 'day'")
            .bind(user_id)
            .fetch_all(pool)
            .await?;

    let mut pages_target = None;
    let mut minutes_target = None;
    for row in rows {
        let kind: String = row.try_get("kind")?;
        let target: i64 = row.try_get("target")?;
        match kind.as_str() {
            GOAL_KIND_PAGES => pages_target = Some(target),
            GOAL_KIND_MINUTES => minutes_target = Some(target),
            // A kind this build doesn't know is skipped rather than rendered:
            // the row is a target for something with no measurement here, and
            // reporting it against a zero would invent progress.
            _ => {}
        }
    }

    let day = user_offset::today(pool, offset_minutes).await?;
    let pages_today = pages::pages_read_on_day(pool, user_id, &day, offset_minutes).await?;
    // Truncating, not rounding: a reader 59 seconds into their first minute
    // has not read a minute yet, and a goal that rounds up hands out progress
    // nobody earned.
    let minutes_today = day_seconds(pool, user_id, offset_minutes).await? / 60;

    let pages = pages_target.map(|target| DailyGoal {
        kind: GOAL_KIND_PAGES.to_string(),
        target,
        current: pages_today,
        day: day.clone(),
    });
    let minutes = minutes_target.map(|target| DailyGoal {
        kind: GOAL_KIND_MINUTES.to_string(),
        target,
        current: minutes_today,
        day,
    });

    Ok(DailyGoals {
        pages,
        minutes,
        // Always zero now, and kept only so a client built against the old shape
        // still decodes. It counted seconds the minutes goal had to *exclude*
        // because the session carried no capture-time offset to place it with;
        // the day now comes from the reader's current offset instead, which
        // every session can be measured against, so nothing is excluded and
        // there is nothing left to disclose.
        unzoned_seconds: 0,
        pages_today: Some(pages_today),
        minutes_today: Some(minutes_today),
    })
}

/// Seconds read and listened today on the reader's calendar.
///
/// **Every** session counts, whether or not it recorded a capture-time offset.
/// That is the difference from `super::patterns`, and it follows from what the
/// two are measuring: an hour-of-day bucket is a claim about where the reader
/// *was*, which an offsetless row cannot support, while a day boundary is a
/// property of where they are *now* — and `started_at` alone is enough to place
/// any row against it. So there is no unplaceable remainder here to disclose.
///
/// Scanned from a bounded tail rather than over the reader's whole history: the
/// day being asked about is at most ±14 hours off the UTC one, so every row that
/// can land on it sits inside [`LOCAL_DAY_SCAN_SECS`], and the bound is what
/// lets this use `(user_id, started_at)` instead of a full scan.
async fn day_seconds(
    pool: &SqlitePool,
    user_id: i64,
    offset_minutes: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COALESCE(SUM(secs), 0)
         FROM ({SESSION_LOCAL_ROWS})
         WHERE {} = {}",
        calendar::local_day("started_at", offset_minutes),
        calendar::local_day("CAST(strftime('%s','now') AS INTEGER)", offset_minutes)
    );
    let start = scan_start(pool).await?;
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

/// The unix second the local-day scan starts from — see
/// [`LOCAL_DAY_SCAN_SECS`]. Resolved in SQLite so it shares the clock every
/// other window boundary is cut on.
async fn scan_start(pool: &SqlitePool) -> Result<i64, StatsError> {
    Ok(
        sqlx::query_scalar("SELECT CAST(strftime('%s','now') AS INTEGER) - ?")
            .bind(LOCAL_DAY_SCAN_SECS)
            .fetch_one(pool)
            .await?,
    )
}

#[cfg(test)]
mod tests;
