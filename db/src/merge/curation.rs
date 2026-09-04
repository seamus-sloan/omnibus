//! Per-reader curation — read status and rating — across a merge and its
//! undo. The merge settles a per-reader collision by *deleting* the losing
//! row, so undo can only put each book back if the merge recorded both sides
//! first; this module is that record and its replay.

use serde::{Deserialize, Serialize};
use sqlx::Transaction;

use super::MergeError;

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

/// Keyed per reader, which is all [`plan_restore`] needs to know about a row.
pub(super) trait CurationRow {
    fn user_id(&self) -> i64;
}

impl CurationRow for ReadStatusRow {
    fn user_id(&self) -> i64 {
        self.user_id
    }
}

impl CurationRow for RatingRow {
    fn user_id(&self) -> i64 {
        self.user_id
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
pub(super) async fn restore_curation(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
    snap: &CurationSnapshot,
) -> Result<(), MergeError> {
    restore_status(tx, source_uuid, target_uuid, snap).await?;
    restore_ratings(tx, source_uuid, target_uuid, snap).await
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
/// Errors with the offending `user_id` when the target no longer carries what
/// the merge left it: the reader has re-curated the survivor since, so putting
/// the pre-merge value back would discard a later decision and leaving it
/// would strand a value that started on the other book. Neither is an answer,
/// so undo refuses (AC4) rather than picking a side.
fn plan_restore<'a, T>(
    source_pre: &'a [T],
    target_pre: &'a [T],
    merged: &'a [T],
    current: &'a [T],
) -> Result<RestorePlan<'a, T>, i64>
where
    T: CurationRow + PartialEq,
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
        if by_user(merged, user) != by_user(current, user) {
            return Err(user);
        }
        plan.to_source.push(row);
        match by_user(target_pre, user) {
            Some(before) => plan.to_target.push(before),
            None => plan.clear_target.push(user),
        }
    }
    Ok(plan)
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
) -> Result<(), MergeError> {
    let current = load_status(tx, target_uuid).await?;
    let plan = plan_restore(
        &snap.source_status,
        &snap.target_status,
        &snap.merged_status,
        &current,
    )
    .map_err(|user| conflict("read status", user))?;

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
) -> Result<(), MergeError> {
    let current = load_ratings(tx, target_uuid).await?;
    let plan = plan_restore(
        &snap.source_ratings,
        &snap.target_ratings,
        &snap.merged_ratings,
        &current,
    )
    .map_err(|user| conflict("rating", user))?;

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

/// The message a refused undo carries. Names the field and the reader so an
/// admin can look at the two rows and decide, which is the whole point of
/// failing instead of guessing.
fn conflict(what: &str, user_id: i64) -> MergeError {
    MergeError::UndoConflict(format!(
        "the surviving book's {what} for user {user_id} changed after the merge"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rating(user_id: i64, half_stars: i64) -> RatingRow {
        RatingRow {
            user_id,
            half_stars,
            updated_at: 1_000,
        }
    }

    #[test]
    fn plan_restore_sends_each_row_back_to_the_book_it_came_from() {
        let source_pre = vec![rating(1, 5)];
        let target_pre = vec![rating(1, 8)];
        // The source's row was the newer one, so the merge left it on the target.
        let merged = vec![rating(1, 5)];
        let plan = plan_restore(&source_pre, &target_pre, &merged, &merged).unwrap();
        assert_eq!(plan.to_source, vec![&source_pre[0]]);
        assert_eq!(plan.to_target, vec![&target_pre[0]]);
        assert!(plan.clear_target.is_empty());
    }

    #[test]
    fn plan_restore_clears_the_target_row_when_the_target_had_none() {
        let source_pre = vec![rating(1, 5)];
        let merged = vec![rating(1, 5)];
        let plan = plan_restore(&source_pre, &[], &merged, &merged).unwrap();
        assert_eq!(plan.to_source, vec![&source_pre[0]]);
        assert!(plan.to_target.is_empty());
        assert_eq!(plan.clear_target, vec![1]);
    }

    #[test]
    fn plan_restore_ignores_readers_the_merge_never_moved_a_row_for() {
        // User 2 rated only the target, so the merge left them alone — and
        // undo must too, even though their row has changed since.
        let target_pre = vec![rating(2, 4)];
        let merged = vec![rating(2, 4)];
        let current = vec![rating(2, 9)];
        let plan = plan_restore(&[], &target_pre, &merged, &current).unwrap();
        assert!(plan.to_source.is_empty());
        assert!(plan.to_target.is_empty());
        assert!(plan.clear_target.is_empty());
    }

    #[test]
    fn plan_restore_refuses_when_the_survivor_was_recurated_after_the_merge() {
        let source_pre = vec![rating(1, 5)];
        let merged = vec![rating(1, 5)];
        let current = vec![rating(1, 9)];
        assert_eq!(
            plan_restore(&source_pre, &[], &merged, &current).unwrap_err(),
            1
        );
    }

    #[test]
    fn plan_restore_refuses_when_the_survivors_row_was_deleted_after_the_merge() {
        let source_pre = vec![rating(1, 5)];
        let merged = vec![rating(1, 5)];
        assert_eq!(plan_restore(&source_pre, &[], &merged, &[]).unwrap_err(), 1);
    }
}
