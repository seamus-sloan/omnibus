//! Per-reader curation — read status and rating — across a merge and its
//! undo. The merge settles a per-reader collision by *deleting* the losing
//! row, so undo can only put each book back if the merge recorded both sides
//! first; this module is that record and its replay.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sqlx::Transaction;

use super::MergeError;

#[cfg(test)]
mod tests;

/// A `book_read_status` row without its book or surrogate id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub(super) struct ReadStatusRow {
    pub user_id: i64,
    pub status: String,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

/// A `user_ratings` row without its book or surrogate id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub(super) struct RatingRow {
    pub user_id: i64,
    pub half_stars: i64,
    pub updated_at: i64,
}

/// Keyed per reader, and comparable on what it *says* rather than when it was
/// written — the two things [`plan_restore`] needs from a row.
pub(super) trait CurationRow {
    fn user_id(&self) -> i64;

    /// Whether two rows make the same statement about the book, ignoring
    /// `updated_at`.
    ///
    /// The timestamp is not part of the statement, and treating it as one
    /// makes undo refuse over nothing: both writers bump `updated_at` on a
    /// re-affirmation of the value already held, and every reading surface
    /// auto-writes `reading` on open — so merely *opening* the merged book
    /// would block its undo.
    fn same_value(&self, other: &Self) -> bool;
}

impl CurationRow for ReadStatusRow {
    fn user_id(&self) -> i64 {
        self.user_id
    }

    fn same_value(&self, other: &Self) -> bool {
        self.status == other.status && self.finished_at == other.finished_at
    }
}

impl CurationRow for RatingRow {
    fn user_id(&self) -> i64 {
        self.user_id
    }

    fn same_value(&self, other: &Self) -> bool {
        self.half_stars == other.half_stars
    }
}

/// Both books' curation rows either side of the merge, carried in the
/// `merge_log` snapshot.
///
/// `source_*` / `target_*` are what each book held **before** the merge;
/// `merged_*` is what the merge left on the target. Undo needs all three: the
/// first two say where each row belongs, and the third is what undo must still
/// find on the survivor to know no reader has re-curated it since.
///
/// Empty on a `merge_log` row written before the merge recorded any of this —
/// undo then leaves curation alone, which is the behaviour those merges were
/// performed under.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct CurationSnapshot {
    pub source_status: Vec<ReadStatusRow>,
    pub target_status: Vec<ReadStatusRow>,
    pub merged_status: Vec<ReadStatusRow>,
    pub source_ratings: Vec<RatingRow>,
    pub target_ratings: Vec<RatingRow>,
    pub merged_ratings: Vec<RatingRow>,
}

impl CurationSnapshot {
    /// Every reader this snapshot moved a row for. Undo of an *earlier* merge
    /// into the same book consults this to find out whose rows it must not
    /// touch — see [`PlanContext::claimed_by_later_merges`].
    pub(in crate::merge) fn moved_readers(&self) -> impl Iterator<Item = i64> + '_ {
        self.source_status
            .iter()
            .map(CurationRow::user_id)
            .chain(self.source_ratings.iter().map(CurationRow::user_id))
    }
}

/// Snapshot both books' curation, before the merge moves or deletes anything.
pub(super) async fn capture_pre(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
) -> Result<CurationSnapshot, sqlx::Error> {
    Ok(CurationSnapshot {
        source_status: load_status(tx, source_uuid).await?,
        target_status: load_status(tx, target_uuid).await?,
        source_ratings: load_ratings(tx, source_uuid).await?,
        target_ratings: load_ratings(tx, target_uuid).await?,
        ..Default::default()
    })
}

/// Record what the merge left on the target, once the retarget has run. This
/// is undo's proof that the survivor is untouched, so it must be read *after*
/// the dedupe and retarget, not before.
pub(super) async fn capture_post(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    target_uuid: &str,
    snap: &mut CurationSnapshot,
) -> Result<(), sqlx::Error> {
    snap.merged_status = load_status(tx, target_uuid).await?;
    snap.merged_ratings = load_ratings(tx, target_uuid).await?;
    Ok(())
}

/// Reverse the merge's per-reader curation move: each book ends up with
/// exactly the read status and rating it carried before.
///
/// `claimed_by_later_merges` comes from the caller because only it knows which
/// `merge_log` rows are still open against this target.
pub(super) async fn restore_curation(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
    snap: &CurationSnapshot,
    claimed_by_later_merges: &HashSet<i64>,
) -> Result<(), MergeError> {
    let ctx = PlanContext {
        live_users: load_live_users(tx).await?,
        claimed_by_later_merges: claimed_by_later_merges.clone(),
    };
    restore_status(tx, source_uuid, target_uuid, snap, &ctx).await?;
    restore_ratings(tx, source_uuid, target_uuid, snap, &ctx).await
}

/// The facts outside one table's four row-sets that decide whether a reader's
/// row can be settled at all.
struct PlanContext {
    /// `users.id`s that still exist. Both tables cascade on `users(id)`, so a
    /// deleted account took its curation off *both* books — there is nothing
    /// to restore and nothing to conflict about, and re-inserting the snapshot
    /// row would violate the foreign key.
    live_users: HashSet<i64>,
    /// Readers a later, still-open merge into the same target also moved a row
    /// for. That merge's own dedupe may have deleted a row this one cannot
    /// see, so rewriting the survivor here would destroy it — and leave the
    /// later merge's undo permanently unable to put it back.
    claimed_by_later_merges: HashSet<i64>,
}

/// Why undo cannot settle one reader's row. Carries the reader so the message
/// can name who to look at.
#[derive(Debug, PartialEq, Eq)]
enum Unresolvable {
    /// The survivor's row no longer says what the merge left it saying.
    Recurated(i64),
    /// A later, still-open merge into the same book also moved this reader's
    /// row.
    ClaimedByLaterMerge(i64),
}

/// Where one table's rows must end up, for the readers the merge moved a row
/// for. Readers it did not move are absent — untouched by the merge, so
/// untouched by its undo.
#[derive(Debug)]
struct RestorePlan<'a, T> {
    /// Rows to write onto the recreated source book.
    to_source: Vec<&'a T>,
    /// Rows to put back on the target …
    to_target: Vec<&'a T>,
    /// … and the readers whose target row must simply go away, because the
    /// target had none of its own before the merge.
    clear_target: Vec<i64>,
}

/// Plan one table's undo from the three snapshot sides plus the survivor's
/// rows as they stand now.
///
/// Errors rather than guessing when a reader's row cannot be settled — the two
/// [`Unresolvable`] cases. Both are AC4's "fail loudly": keeping the pre-merge
/// value would discard a later decision, and keeping the current one would
/// strand a value that started on the other book.
fn plan_restore<'a, T>(
    source_pre: &'a [T],
    target_pre: &'a [T],
    merged: &'a [T],
    current: &'a [T],
    ctx: &PlanContext,
) -> Result<RestorePlan<'a, T>, Unresolvable>
where
    T: CurationRow,
{
    let by_user = |rows: &'a [T], user: i64| rows.iter().find(|r| r.user_id() == user);

    let mut plan = RestorePlan {
        to_source: Vec::new(),
        to_target: Vec::new(),
        clear_target: Vec::new(),
    };
    // Only readers who had a row on the *source* are in scope: those are the
    // ones whose row the merge relocated, and a reader who curated the target
    // alone was never touched.
    for row in source_pre {
        let user = row.user_id();
        if !ctx.live_users.contains(&user) {
            continue;
        }
        if ctx.claimed_by_later_merges.contains(&user) {
            return Err(Unresolvable::ClaimedByLaterMerge(user));
        }
        if !says_the_same(by_user(merged, user), by_user(current, user)) {
            return Err(Unresolvable::Recurated(user));
        }
        plan.to_source.push(row);
        match by_user(target_pre, user) {
            Some(before) => plan.to_target.push(before),
            None => plan.clear_target.push(user),
        }
    }
    Ok(plan)
}

/// Whether two optional rows make the same statement — including both being
/// absent. A row that has since been *cleared* is a change like any other.
fn says_the_same<T: CurationRow>(a: Option<&T>, b: Option<&T>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.same_value(y),
        (None, None) => true,
        _ => false,
    }
}

/// `users.id`s that still exist.
async fn load_live_users(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<HashSet<i64>, sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM users")
        .fetch_all(&mut **tx)
        .await?;
    Ok(ids.into_iter().collect())
}

/// Read one book's `book_read_status` rows, reader-ordered.
async fn load_status(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_uuid: &str,
) -> Result<Vec<ReadStatusRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, status, updated_at, finished_at FROM book_read_status
          WHERE book_uuid = ? ORDER BY user_id",
    )
    .bind(book_uuid)
    .fetch_all(&mut **tx)
    .await
}

/// Read one book's `user_ratings` rows, reader-ordered.
async fn load_ratings(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_uuid: &str,
) -> Result<Vec<RatingRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, half_stars, updated_at FROM user_ratings
          WHERE book_uuid = ? ORDER BY user_id",
    )
    .bind(book_uuid)
    .fetch_all(&mut **tx)
    .await
}

/// Apply the read-status half of the undo.
async fn restore_status(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
    snap: &CurationSnapshot,
    ctx: &PlanContext,
) -> Result<(), MergeError> {
    let current = load_status(tx, target_uuid).await?;
    let plan = plan_restore(
        &snap.source_status,
        &snap.target_status,
        &snap.merged_status,
        &current,
        ctx,
    )
    .map_err(|why| conflict("read status", &why))?;

    for user in plan.clear_target {
        sqlx::query("DELETE FROM book_read_status WHERE user_id = ? AND book_uuid = ?")
            .bind(user)
            .bind(target_uuid)
            .execute(&mut **tx)
            .await?;
    }
    for (book_uuid, rows) in [(target_uuid, plan.to_target), (source_uuid, plan.to_source)] {
        for row in rows {
            sqlx::query(
                "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(user_id, book_uuid) DO UPDATE SET
                   status = excluded.status,
                   updated_at = excluded.updated_at,
                   finished_at = excluded.finished_at",
            )
            .bind(row.user_id)
            .bind(book_uuid)
            .bind(&row.status)
            .bind(row.updated_at)
            .bind(row.finished_at)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

/// Apply the rating half of the undo.
async fn restore_ratings(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
    snap: &CurationSnapshot,
    ctx: &PlanContext,
) -> Result<(), MergeError> {
    let current = load_ratings(tx, target_uuid).await?;
    let plan = plan_restore(
        &snap.source_ratings,
        &snap.target_ratings,
        &snap.merged_ratings,
        &current,
        ctx,
    )
    .map_err(|why| conflict("rating", &why))?;

    for user in plan.clear_target {
        sqlx::query("DELETE FROM user_ratings WHERE user_id = ? AND book_uuid = ?")
            .bind(user)
            .bind(target_uuid)
            .execute(&mut **tx)
            .await?;
    }
    for (book_uuid, rows) in [(target_uuid, plan.to_target), (source_uuid, plan.to_source)] {
        for row in rows {
            sqlx::query(
                "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(user_id, book_uuid) DO UPDATE SET
                   half_stars = excluded.half_stars,
                   updated_at = excluded.updated_at",
            )
            .bind(row.user_id)
            .bind(book_uuid)
            .bind(row.half_stars)
            .bind(row.updated_at)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

/// The message a refused undo carries. Names the field, the reader and which
/// of the two obstacles it hit — an admin has to look at the actual rows to
/// settle either, so the message says where to look.
fn conflict(what: &str, why: &Unresolvable) -> MergeError {
    MergeError::UndoConflict(match why {
        Unresolvable::Recurated(user) => {
            format!("the surviving book's {what} for user {user} changed after the merge")
        }
        Unresolvable::ClaimedByLaterMerge(user) => format!(
            "a later merge into the surviving book also moved the {what} for user {user}; \
             undo that merge first"
        ),
    })
}
