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

use super::{compute, pages, patterns::SESSION_LOCAL_ROWS, StatsError};

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

/// The server's current calendar year (UTC). Read from SQLite rather than the
/// process clock so it lands on the same "now" every other stats window does.
pub async fn current_year(pool: &SqlitePool) -> Result<i64, StatsError> {
    Ok(
        sqlx::query_scalar("SELECT CAST(strftime('%Y', 'now') AS INTEGER)")
            .fetch_one(pool)
            .await?,
    )
}

/// Unix-second bounds `[start, end)` of `year` in UTC.
async fn year_bounds(pool: &SqlitePool, year: i64) -> Result<(i64, i64), StatsError> {
    let row = sqlx::query(
        "SELECT CAST(strftime('%s', ? || '-01-01 00:00:00') AS INTEGER) AS lo,
                CAST(strftime('%s', ? || '-01-01 00:00:00') AS INTEGER) AS hi",
    )
    .bind(format!("{year:04}"))
    .bind(format!("{:04}", year + 1))
    .fetch_one(pool)
    .await?;
    Ok((row.try_get("lo")?, row.try_get("hi")?))
}

/// The caller's goal for the current calendar year, `None` when unset — the
/// value `compute` hangs off [`omnibus_shared::StatsSummary::goal`].
pub(super) async fn current_goal(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Option<ReadingGoal>, StatsError> {
    let year = current_year(pool).await?;
    goal_for_year(pool, user_id, year).await
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
) -> Result<Option<ReadingGoal>, StatsError> {
    let Some(row) = sqlx::query(
        "SELECT kind, target FROM reading_goals
         WHERE user_id = ? AND scope = 'year' AND year = ? AND kind = ?",
    )
    .bind(user_id)
    .bind(year)
    .bind(GOAL_KIND_BOOKS)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let (start, end) = year_bounds(pool, year).await?;
    let current = compute::finished_count_bounded(pool, user_id, start, end).await?;
    Ok(Some(ReadingGoal {
        kind: row.try_get("kind")?,
        target: row.try_get("target")?,
        current,
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
) -> Result<Option<ReadingGoal>, GoalError> {
    let kind = update.kind_or_default();
    if kind != GOAL_KIND_BOOKS {
        return Err(GoalError::UnsupportedKind(kind.to_string()));
    }
    let year = match update.year {
        Some(year) => year,
        None => current_year(pool).await?,
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
    Ok(goal_for_year(pool, user_id, year).await?)
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
    Ok(daily_goals(pool, user_id).await?)
}

/// The caller's standing daily goals with today's progress toward each.
///
/// Each kind's progress is measured **only when that kind has a target**. The
/// two measurements are unrelated queries — one over the pages ledger, one
/// over the session tables — and running the ledger scan for a reader who only
/// set a minutes goal would be work with nowhere to go.
///
/// # Which day each kind is measured over
///
/// Not the same one, and that is deliberate rather than an oversight.
/// **Minutes** use the reader's *local* day, from the capture-time offset each
/// session recorded (migration `0080`) — the treatment [`super::patterns`]
/// already gives the time-of-day strips, and the only honest one for a target
/// that resets at midnight. **Pages** use the **UTC** day, because the
/// forward-progress ledger (migration `0083`) buckets to a UTC date string and
/// retains no timestamp to re-bucket from. The two can therefore name different
/// days for the same moment, by up to the reader's offset; closing that gap
/// means teaching the ledger an offset, which moves the progress write path on
/// both clients.
pub async fn daily_goals(pool: &SqlitePool, user_id: i64) -> Result<DailyGoals, StatsError> {
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

    // Nothing set means nothing to measure, and nothing to ask the clock for.
    if pages_target.is_none() && minutes_target.is_none() {
        return Ok(DailyGoals::default());
    }
    let utc = utc_today(pool).await?;

    let pages = match pages_target {
        Some(target) => Some(DailyGoal {
            kind: GOAL_KIND_PAGES.to_string(),
            target,
            current: pages::pages_read_on_day(pool, user_id, &utc).await?,
            day: utc.clone(),
        }),
        None => None,
    };

    let (minutes, unzoned_seconds) = match minutes_target {
        Some(target) => {
            let day = local_today(pool, user_id).await?;
            let (seconds, unzoned) = day_seconds(pool, user_id, &day, &utc).await?;
            (
                Some(DailyGoal {
                    kind: GOAL_KIND_MINUTES.to_string(),
                    target,
                    // Truncating, not rounding: a reader 59 seconds into their
                    // first minute has not read a minute yet, and a goal that
                    // rounds up hands out progress nobody earned.
                    current: seconds / 60,
                    day,
                }),
                unzoned,
            )
        }
        None => (None, 0),
    };

    Ok(DailyGoals {
        pages,
        minutes,
        unzoned_seconds,
    })
}

/// Today's UTC date as `YYYY-MM-DD` — the calendar the pages ledger buckets on.
async fn utc_today(pool: &SqlitePool) -> Result<String, StatsError> {
    Ok(sqlx::query_scalar("SELECT date('now')")
        .fetch_one(pool)
        .await?)
}

/// Today's date on the reader's own clock, as `YYYY-MM-DD`.
///
/// The offset comes from their **most recent** session that recorded one, so a
/// reader who has moved gets the day where they are now rather than where they
/// were. Falling back to UTC when no session ever recorded an offset is the
/// only option available — and it is the honest one, since a pre-`0080`
/// account has told the server nothing about where it is.
async fn local_today(pool: &SqlitePool, user_id: i64) -> Result<String, StatsError> {
    let offset = current_offset_minutes(pool, user_id).await?.unwrap_or(0);
    Ok(
        sqlx::query_scalar("SELECT date(strftime('%s','now') + ? * 60, 'unixepoch')")
            .bind(offset)
            .fetch_one(pool)
            .await?,
    )
}

/// The reader's current UTC offset in minutes, from the most recent session
/// carrying one. `None` when no session ever has.
///
/// One indexed probe per table, compared here, rather than one `ORDER BY` over
/// a `UNION ALL` of both: SQLite cannot push the sort through the union, so
/// that shape materialises and sorts a reader's whole offset-carrying session
/// history on a read path that runs for every stats load. Each probe instead
/// walks `idx_{reading,listening}_sessions_user_started` backwards and stops at
/// the first row it wants.
async fn current_offset_minutes(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Option<i64>, StatsError> {
    let mut latest: Option<(i64, i64)> = None;
    for table in ["reading_sessions", "listening_sessions"] {
        // `table` is one of two fixed literals chosen here, never user input.
        let sql = format!(
            "SELECT started_at, utc_offset_minutes FROM {table}
             WHERE user_id = ? AND utc_offset_minutes IS NOT NULL
             ORDER BY started_at DESC LIMIT 1"
        );
        let Some(row) = sqlx::query(&sql).bind(user_id).fetch_optional(pool).await? else {
            continue;
        };
        let candidate: (i64, i64) = (
            row.try_get("started_at")?,
            row.try_get("utc_offset_minutes")?,
        );
        if latest.is_none_or(|(seen, _)| candidate.0 > seen) {
            latest = Some(candidate);
        }
    }
    Ok(latest.map(|(_, offset)| offset))
}

/// `(placed_seconds, unzoned_seconds)` for the reader's day.
///
/// Each session is bucketed by **its own** captured offset, not by the
/// reader's current one — the same rule [`super::patterns`] follows, and what
/// keeps an evening read in Tokyo on the Tokyo day after the reader flies home.
///
/// The two halves are counted against **different calendars**, and have to be.
/// `local_day` places the sessions that recorded an offset. A session that did
/// not record one cannot be placed on any local day at all, so the disclosure
/// falls back to `utc_day` — the only calendar such a row has. Comparing an
/// offsetless session against the local day instead would silently report zero
/// for any reader whose offset pushes their day off the UTC one, which is
/// every reader the disclosure exists for.
async fn day_seconds(
    pool: &SqlitePool,
    user_id: i64,
    local_day: &str,
    utc_day: &str,
) -> Result<(i64, i64), StatsError> {
    let sql = format!(
        "SELECT COALESCE(SUM(CASE WHEN utc_offset_minutes IS NOT NULL
                                   AND date(started_at + utc_offset_minutes * 60, 'unixepoch') = ?
                                  THEN secs END), 0) AS placed,
                COALESCE(SUM(CASE WHEN utc_offset_minutes IS NULL
                                   AND date(started_at, 'unixepoch') = ?
                                  THEN secs END), 0) AS unzoned
         FROM ({SESSION_LOCAL_ROWS})"
    );
    let start = scan_start(pool).await?;
    let row = sqlx::query(&sql)
        .bind(local_day)
        .bind(utc_day)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?;
    Ok((row.try_get("placed")?, row.try_get("unzoned")?))
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
