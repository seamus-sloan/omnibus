//! Active-day and streak aggregation for the stats page. Sessions bucket to
//! unix day numbers (`started_at / 86400`), so consecutiveness is an integer
//! diff and no date crate is needed. One sorted day list yields all three
//! figures: days active, the longest run, and the run still live today.

use sqlx::SqlitePool;

use super::StatsError;

/// The day-run figures one window yields, all derived from the same sorted
/// distinct active-day list.
#[derive(Debug)]
pub(super) struct Streak {
    pub active_days: i64,
    pub longest_days: i64,
    pub current_days: i64,
}

/// Days active, longest consecutive run, and the run still live at `today`.
///
/// `today` is the server's own day number (from [`super::compute::as_of`]),
/// never a client's — the current streak is a statement about the server's
/// calendar, and the web page, the iOS tab, and any widget must not each
/// derive their own answer and disagree.
///
/// **`active_days` and the longest run are windowed; the current run is
/// deliberately not.** A streak is a fact about right now, not about a
/// reporting period: windowing it would make "your current streak" read 2 on
/// the 2nd of the month for a reader 40 days deep, and 0 on the 1st. The web
/// card happens to render the all-time summary and the iOS tile the
/// period-scoped one, so a windowed value would also make the two clients
/// disagree about a field that exists to stop exactly that.
///
/// **UTC, like every other day figure in `db::stats`.** No user timezone is
/// stored anywhere in the schema, so a reader at UTC-7 reading at 21:00 has
/// that session filed on the following calendar day — which can both fabricate
/// and break a streak. Fixing that means local-time bucketing, which moves the
/// heatmap, active days, and every window boundary too; it is not this
/// metric's to fix alone.
pub(super) async fn streak(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    today: i64,
) -> Result<Streak, StatsError> {
    let windowed = active_days(pool, user_id, start).await?;
    // `start = 0` — every day on record, so the live run is the reader's real
    // one whatever period the page is showing.
    let all_time = if start == 0 {
        None
    } else {
        Some(active_days(pool, user_id, 0).await?)
    };

    Ok(Streak {
        active_days: windowed.len() as i64,
        longest_days: longest_run(&windowed),
        current_days: current_run(all_time.as_ref().unwrap_or(&windowed), today),
    })
}

/// The user's distinct active days (reading or listening) from `start`,
/// ascending, as unix day numbers.
async fn active_days(pool: &SqlitePool, user_id: i64, start: i64) -> Result<Vec<i64>, StatsError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT started_at / 86400 AS dnum FROM (
             SELECT started_at FROM reading_sessions   WHERE user_id = ? AND started_at >= ?
             UNION ALL
             SELECT started_at FROM listening_sessions WHERE user_id = ? AND started_at >= ?
         ) ORDER BY dnum",
    )
    .bind(user_id)
    .bind(start)
    .bind(user_id)
    .bind(start)
    .fetch_all(pool)
    .await?)
}

/// Longest run of consecutive days in a sorted, distinct day list.
fn longest_run(days: &[i64]) -> i64 {
    let mut longest = if days.is_empty() { 0 } else { 1 };
    let mut run = longest;
    for pair in days.windows(2) {
        run = if pair[1] - pair[0] == 1 { run + 1 } else { 1 };
        longest = longest.max(run);
    }
    longest
}

/// The run still live at `today`, counted backwards from the last active day.
///
/// A run ending **yesterday** is still live: a reader who hasn't read *yet*
/// today keeps their streak until the day is over, and collapsing it the
/// instant the date rolls over is what makes a naive streak feel broken. A run
/// ending any earlier is finished, and reports zero.
fn current_run(days: &[i64], today: i64) -> i64 {
    // Drop anything dated after today first. `SessionReport::validate` bounds
    // `started_at` below but not above, and the server never restamps it, so a
    // device with a fast clock can file a session in the future. Left in, it
    // becomes the head of the run and the gap *behind* it breaks the walk
    // immediately — reporting 1 for a reader who is 40 days deep.
    let days = match days.iter().rposition(|&d| d <= today) {
        Some(i) => &days[..=i],
        None => return 0,
    };
    let Some(&last) = days.last() else {
        return 0;
    };
    if last < today - 1 {
        return 0;
    }
    let mut run = 1;
    for pair in days.windows(2).rev() {
        if pair[1] - pair[0] != 1 {
            break;
        }
        run += 1;
    }
    run
}

#[cfg(test)]
mod tests;
