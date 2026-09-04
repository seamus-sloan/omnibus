//! Tests for the curation snapshot and its replay: the pure planner, then
//! merge-then-undo end to end for read status, ratings and the identifier
//! tuples that travel with them.

use super::*;
use crate::merge::{merge_books, undo_merge, MergeError};
use crate::pool::init_db;
use crate::test_support::{
    count_rows as count, seed_synced_audiobook as seed_audiobook, seed_synced_ebook as seed_ebook,
    seed_user,
};

// --- the planner, in isolation ---

fn rating(user_id: i64, half_stars: i64, updated_at: i64) -> RatingRow {
    RatingRow {
        user_id,
        half_stars,
        updated_at,
    }
}

/// A context that gets in the planner's way as little as possible: `user` is a
/// live account, and no later merge is holding anything.
fn plain_ctx(user: i64) -> PlanContext {
    PlanContext {
        live_users: HashSet::from([user]),
        claimed_by_later_merges: HashSet::new(),
    }
}

#[test]
fn plan_restore_sends_each_row_back_to_the_book_it_came_from() {
    let source_pre = vec![rating(1, 5, 2_000)];
    let target_pre = vec![rating(1, 8, 1_000)];
    // The source's row was the newer one, so the merge left it on the target.
    let merged = vec![rating(1, 5, 2_000)];
    let plan = plan_restore(&source_pre, &target_pre, &merged, &merged, &plain_ctx(1)).unwrap();
    assert_eq!(plan.to_source, vec![&source_pre[0]]);
    assert_eq!(plan.to_target, vec![&target_pre[0]]);
    assert!(plan.clear_target.is_empty());
}

#[test]
fn plan_restore_clears_the_target_row_when_the_target_had_none() {
    let source_pre = vec![rating(1, 5, 2_000)];
    let merged = vec![rating(1, 5, 2_000)];
    let plan = plan_restore(&source_pre, &[], &merged, &merged, &plain_ctx(1)).unwrap();
    assert_eq!(plan.to_source, vec![&source_pre[0]]);
    assert!(plan.to_target.is_empty());
    assert_eq!(plan.clear_target, vec![1]);
}

#[test]
fn plan_restore_ignores_readers_the_merge_never_moved_a_row_for() {
    // User 2 rated only the target, so the merge left them alone — and undo
    // must too, even though their row has changed since.
    let target_pre = vec![rating(2, 4, 1_000)];
    let merged = vec![rating(2, 4, 1_000)];
    let current = vec![rating(2, 9, 5_000)];
    let plan = plan_restore(&[], &target_pre, &merged, &current, &plain_ctx(2)).unwrap();
    assert!(plan.to_source.is_empty());
    assert!(plan.to_target.is_empty());
    assert!(plan.clear_target.is_empty());
}

#[test]
fn plan_restore_refuses_when_the_survivor_was_recurated_after_the_merge() {
    let source_pre = vec![rating(1, 5, 2_000)];
    let merged = vec![rating(1, 5, 2_000)];
    let current = vec![rating(1, 9, 5_000)];
    assert_eq!(
        plan_restore(&source_pre, &[], &merged, &current, &plain_ctx(1)).unwrap_err(),
        Unresolvable::Recurated(1)
    );
}

#[test]
fn plan_restore_refuses_when_the_survivors_row_was_deleted_after_the_merge() {
    let source_pre = vec![rating(1, 5, 2_000)];
    let merged = vec![rating(1, 5, 2_000)];
    assert_eq!(
        plan_restore(&source_pre, &[], &merged, &[], &plain_ctx(1)).unwrap_err(),
        Unresolvable::Recurated(1)
    );
}

#[test]
fn plan_restore_allows_a_timestamp_only_touch_of_the_same_value() {
    // Both writers bump `updated_at` on a re-affirmation, and every reading
    // surface auto-writes `reading` on open — so comparing whole rows would
    // let merely opening the merged book block its undo.
    let source_pre = vec![rating(1, 5, 2_000)];
    let merged = vec![rating(1, 5, 2_000)];
    let current = vec![rating(1, 5, 9_000)];
    let plan = plan_restore(&source_pre, &[], &merged, &current, &plain_ctx(1)).unwrap();
    assert_eq!(plan.clear_target, vec![1]);
}

#[test]
fn plan_restore_skips_a_reader_whose_account_is_gone() {
    // Both tables cascade on `users(id)`, so a deleted account took its rows
    // off both books. There is nothing to restore and nothing to conflict
    // about — and re-inserting would violate the foreign key.
    let source_pre = vec![rating(1, 5, 2_000)];
    let merged = vec![rating(1, 5, 2_000)];
    let ctx = PlanContext {
        live_users: HashSet::new(),
        claimed_by_later_merges: HashSet::new(),
    };
    let plan = plan_restore(&source_pre, &[], &merged, &[], &ctx).unwrap();
    assert!(plan.to_source.is_empty());
    assert!(plan.clear_target.is_empty());
}

#[test]
fn plan_restore_refuses_a_reader_a_later_open_merge_also_moved() {
    let source_pre = vec![rating(1, 5, 2_000)];
    let merged = vec![rating(1, 5, 2_000)];
    let ctx = PlanContext {
        live_users: HashSet::from([1]),
        claimed_by_later_merges: HashSet::from([1]),
    };
    assert_eq!(
        plan_restore(&source_pre, &[], &merged, &merged, &ctx).unwrap_err(),
        Unresolvable::ClaimedByLaterMerge(1)
    );
}

#[test]
fn read_status_says_the_same_ignores_updated_at_but_not_finished_at() {
    let row = |status: &str, updated_at, finished_at| ReadStatusRow {
        user_id: 1,
        status: status.into(),
        updated_at,
        finished_at,
    };
    assert!(row("reading", 1, None).same_value(&row("reading", 9, None)));
    assert!(!row("reading", 1, None).same_value(&row("finished", 1, Some(5))));
    // When the reader finished is part of the statement, not bookkeeping.
    assert!(!row("finished", 1, Some(5)).same_value(&row("finished", 1, Some(9))));
}

// --- merge then undo, end to end ---

/// Stamp the two fields a merge settles per reader — read status and rating —
/// on `book_uuid`, both carrying `ts` as their `updated_at` so a test can say
/// which side the merge's latest-wins dedupe should keep.
async fn seed_curation(
    pool: &sqlx::SqlitePool,
    user: i64,
    book_uuid: &str,
    status: &str,
    half_stars: i64,
    ts: i64,
) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(book_uuid)
    .bind(status)
    .bind(ts)
    .bind((status == "finished").then_some(ts))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(user)
    .bind(book_uuid)
    .bind(half_stars)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
}

/// One reader's whole curation on a book — `(status, finished_at, status
/// updated_at, half_stars, rating updated_at)`, each `None` where the row is
/// absent. Every field is compared, so a restore that recovers the status but
/// loses when the reader finished still fails the assertion.
type Curation = (
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

async fn curation_of(pool: &sqlx::SqlitePool, user: i64, book_uuid: &str) -> Curation {
    let status: Option<(String, Option<i64>, i64)> = sqlx::query_as(
        "SELECT status, finished_at, updated_at FROM book_read_status
          WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(book_uuid)
    .fetch_optional(pool)
    .await
    .unwrap();
    let rating: Option<(i64, i64)> = sqlx::query_as(
        "SELECT half_stars, updated_at FROM user_ratings WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(book_uuid)
    .fetch_optional(pool)
    .await
    .unwrap();
    (
        status.as_ref().map(|s| s.0.clone()),
        status.as_ref().and_then(|s| s.1),
        status.as_ref().map(|s| s.2),
        rating.map(|r| r.0),
        rating.map(|r| r.1),
    )
}

/// The curation `seed_curation(_, _, _, status, half_stars, ts)` produced.
fn curated(status: &str, half_stars: i64, ts: i64) -> Curation {
    (
        Some(status.to_string()),
        (status == "finished").then_some(ts),
        Some(ts),
        Some(half_stars),
        Some(ts),
    )
}

const NONE: Curation = (None, None, None, None, None);

#[tokio::test]
async fn undo_merge_returns_curation_to_each_book_when_the_absorbed_side_won() {
    // #2234: undo left curation wherever the merge had put it, so the survivor
    // kept the absorbed book's read status and the absorbed book came back
    // carrying none. Both sides hold a *different* non-null status and rating
    // here — the case that exposed it — with the absorbed book's the newer, so
    // the merge's dedupe deletes the survivor's own rows.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_curation(&pool, user, &target, "reading", 8, 1_000).await;
    seed_curation(&pool, user, &source, "finished", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    // The merge is unchanged: the newer side wins on the survivor.
    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("finished", 5, 2_000)
    );

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("reading", 8, 1_000),
        "the survivor must get its own pre-merge curation back, timestamps included"
    );
    assert_eq!(
        curation_of(&pool, user, &source).await,
        curated("finished", 5, 2_000),
        "the restored book must come back with its own, not with nothing"
    );
}

#[tokio::test]
async fn undo_merge_recreates_curation_the_merge_deleted_when_the_survivor_won() {
    // The mirror direction, and the harder one: the survivor's rows were newer,
    // so the merge *deleted* the absorbed book's outright. Undo can only put
    // them back from the snapshot, which is why the merge has to record both
    // sides rather than just the surviving one.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_curation(&pool, user, &target, "reading", 8, 3_000).await;
    seed_curation(&pool, user, &source, "finished", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("reading", 8, 3_000)
    );

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("reading", 8, 3_000)
    );
    assert_eq!(
        curation_of(&pool, user, &source).await,
        curated("finished", 5, 2_000)
    );
}

#[tokio::test]
async fn undo_merge_clears_the_survivors_curation_when_only_the_absorbed_book_had_any() {
    // Nothing to restore on the survivor means the row has to *go*, not stay:
    // leaving it is exactly how the absorbed book's read status ended up
    // permanently attributed to the wrong book.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_curation(&pool, user, &source, "finished", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(curation_of(&pool, user, &target).await, NONE);
    assert_eq!(
        curation_of(&pool, user, &source).await,
        curated("finished", 5, 2_000)
    );
}

#[tokio::test]
async fn undo_merge_leaves_a_reader_the_merge_never_moved_a_row_for_alone() {
    // A reader who curated only the survivor was untouched by the merge, so its
    // undo must not touch them either — including when they have curated it
    // since, which is not a conflict because nothing of theirs ever moved.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mover = seed_user(&pool, "mover").await;
    let bystander = seed_user(&pool, "bystander").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_curation(&pool, mover, &source, "finished", 5, 2_000).await;
    seed_curation(&pool, bystander, &target, "reading", 3, 1_000).await;

    let out = merge_books(&pool, &source, &target, Some(mover))
        .await
        .unwrap();
    sqlx::query("UPDATE user_ratings SET half_stars = 10 WHERE user_id = ?")
        .bind(bystander)
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(
        curation_of(&pool, bystander, &target).await,
        (
            Some("reading".to_string()),
            None,
            Some(1_000),
            Some(10),
            Some(1_000)
        )
    );
    assert_eq!(curation_of(&pool, mover, &target).await, NONE);
}

#[tokio::test]
async fn undo_merge_survives_a_reader_whose_account_was_deleted_after_the_merge() {
    // Both tables cascade on `users(id)`, so deleting the account took the row
    // off the survivor. Treating that as a re-curation would refuse the undo
    // forever, naming a user that no longer exists — and before the curation
    // snapshot existed this undo simply worked.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "departing").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    seed_curation(&pool, user, &source, "finished", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_read_status").await,
        0,
        "the departed reader's rows must not be resurrected against a dead FK"
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM user_ratings").await, 0);
}

#[tokio::test]
async fn undo_merge_allows_a_timestamp_only_touch_of_the_survivor() {
    // Opening the merged book auto-writes `reading`, which bumps `updated_at`
    // without deciding anything. Refusing there would make undo unusable in
    // the ordinary case of looking at the book you just merged.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    seed_curation(&pool, user, &target, "reading", 8, 1_000).await;
    seed_curation(&pool, user, &source, "reading", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    sqlx::query("UPDATE book_read_status SET updated_at = 9000 WHERE book_uuid = ?")
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE user_ratings SET updated_at = 9000 WHERE book_uuid = ?")
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("reading", 8, 1_000)
    );
    assert_eq!(
        curation_of(&pool, user, &source).await,
        curated("reading", 5, 2_000)
    );
}

#[tokio::test]
async fn undo_merge_refuses_when_the_survivors_read_status_was_recurated() {
    // The reader marked the merged book finished, then asked to undo. The
    // pre-merge value and that later decision are both real and undo has no
    // basis to choose, so it fails loudly and changes nothing.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_curation(&pool, user, &target, "reading", 8, 1_000).await;
    seed_curation(&pool, user, &source, "unread", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    sqlx::query("UPDATE book_read_status SET status = 'finished' WHERE book_uuid = ?")
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    let err = undo_merge(&pool, out.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::UndoConflict(_)), "got {err:?}");
    assert!(err.to_string().contains("read status"), "got {err}");

    // The whole undo rolled back — the source book is still absorbed and the
    // log is still open, so the admin can resolve the two rows and retry.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    let undone: Option<i64> = sqlx::query_scalar("SELECT undone_at FROM merge_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(undone.is_none());
}

#[tokio::test]
async fn undo_merge_refuses_when_only_the_survivors_rating_was_recurated() {
    // The rating half has its own planner call and its own message; a refusal
    // that only ever fires for read status would leave it unproven.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_curation(&pool, user, &target, "reading", 8, 1_000).await;
    seed_curation(&pool, user, &source, "reading", 5, 2_000).await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();
    // Read status left exactly as the merge produced it, so only the rating
    // planner can raise.
    sqlx::query("UPDATE user_ratings SET half_stars = 2 WHERE book_uuid = ?")
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    let err = undo_merge(&pool, out.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::UndoConflict(_)), "got {err:?}");
    assert!(err.to_string().contains("rating"), "got {err}");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

#[tokio::test]
async fn undo_merge_refuses_when_a_later_open_merge_moved_the_same_readers_row() {
    // Two books merged into one. The second merge's dedupe deleted its own
    // source's row, which the first merge's snapshot cannot see — so undoing
    // the first would clear the survivor's row and destroy a value that then
    // exists nowhere, with the second undo permanently unable to restore it.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let first = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let second = seed_ebook(&pool, "C/Dracula rev.epub", "Draculla", "Bram Stoker").await;

    seed_curation(&pool, user, &first, "finished", 10, 3_000).await;
    seed_curation(&pool, user, &second, "reading", 4, 1_000).await;

    let one = merge_books(&pool, &first, &target, Some(user))
        .await
        .unwrap();
    merge_books(&pool, &second, &target, Some(user))
        .await
        .unwrap();

    let err = undo_merge(&pool, one.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::UndoConflict(_)), "got {err:?}");
    assert!(err.to_string().contains("later merge"), "got {err}");

    // Nothing was destroyed: the survivor still carries the winning row, and
    // the second merge's own undo is still available to recover the other.
    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("finished", 10, 3_000)
    );
}

// --- identifiers ---

/// Stamp an identifier tuple on the book with `book_uuid`.
async fn seed_identifier(pool: &sqlx::SqlitePool, book_uuid: &str, scheme: &str, value: &str) {
    sqlx::query(
        "INSERT INTO book_identifiers (book_id, scheme, value)
         VALUES ((SELECT id FROM books WHERE uuid = ?), ?, ?)",
    )
    .bind(book_uuid)
    .bind(scheme)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

/// A book's `(scheme, value)` identifier tuples, ordered for comparison.
async fn identifiers_of(pool: &sqlx::SqlitePool, book_uuid: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT scheme, value FROM book_identifiers
          WHERE book_id = (SELECT id FROM books WHERE uuid = ?)
          ORDER BY scheme, value",
    )
    .bind(book_uuid)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn ident(scheme: &str, value: &str) -> (String, String) {
    (scheme.to_string(), value.to_string())
}

#[tokio::test]
async fn undo_merge_takes_back_only_the_identifiers_the_merge_added() {
    // #2234: the absorbed book's ISBN stayed stamped on the survivor forever,
    // where the check-in exact-identifier rung then resolved it to the wrong
    // book. The shared `amazon` tuple is the control — it was on the survivor
    // already, so the merge did not add it and undo has no claim on it.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_identifier(&pool, &target, "isbn", "111").await;
    seed_identifier(&pool, &target, "amazon", "shared").await;
    seed_identifier(&pool, &source, "isbn", "999").await;
    seed_identifier(&pool, &source, "amazon", "shared").await;

    let out = merge_books(&pool, &source, &target, None).await.unwrap();
    assert_eq!(
        identifiers_of(&pool, &target).await,
        vec![
            ident("amazon", "shared"),
            ident("isbn", "111"),
            ident("isbn", "999")
        ]
    );

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(
        identifiers_of(&pool, &target).await,
        vec![ident("amazon", "shared"), ident("isbn", "111")],
        "the absorbed book's ISBN must not outlive the merge on the survivor"
    );
    assert_eq!(
        identifiers_of(&pool, &source).await,
        vec![ident("amazon", "shared"), ident("isbn", "999")]
    );
}

#[tokio::test]
async fn undo_merge_keeps_an_identifier_a_later_open_merge_still_supplies() {
    // Two copies of one edition merged into a third book, both carrying the
    // same ISBN. Only the first merge records it as *added*; stripping it on
    // that undo would leave the survivor with no ISBN despite still holding
    // the second book, and stop the check-in rung resolving it.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let first = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let second = seed_ebook(&pool, "C/Dracula rev.epub", "Draculla", "Bram Stoker").await;
    seed_identifier(&pool, &first, "isbn", "9780000000001").await;
    seed_identifier(&pool, &second, "isbn", "9780000000001").await;

    let one = merge_books(&pool, &first, &target, None).await.unwrap();
    merge_books(&pool, &second, &target, None).await.unwrap();

    undo_merge(&pool, one.merge_log_id).await.unwrap();

    assert_eq!(
        identifiers_of(&pool, &target).await,
        vec![ident("isbn", "9780000000001")],
        "the still-absorbed second book keeps supplying the ISBN"
    );
    assert_eq!(
        identifiers_of(&pool, &first).await,
        vec![ident("isbn", "9780000000001")]
    );
}

#[tokio::test]
async fn undo_merge_leaves_curation_and_identifiers_alone_for_an_older_merge_log() {
    // `curation` / `identifiers_added_to_target` are `#[serde(default)]`, so a
    // merge recorded before they existed still replays — undoing it leaves both
    // where the old undo left them rather than failing to decode.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    seed_curation(&pool, user, &source, "finished", 5, 2_000).await;
    seed_identifier(&pool, &source, "isbn", "999").await;

    let out = merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let json: String = sqlx::query_scalar("SELECT source_metadata FROM merge_log WHERE id = ?")
        .bind(out.merge_log_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = value.as_object_mut().unwrap();
    assert!(obj.remove("curation").is_some());
    assert!(obj.remove("identifiers_added_to_target").is_some());
    sqlx::query("UPDATE merge_log SET source_metadata = ? WHERE id = ?")
        .bind(value.to_string())
        .bind(out.merge_log_id)
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(
        curation_of(&pool, user, &target).await,
        curated("finished", 5, 2_000),
        "an old snapshot carries no curation to restore, so it stays put"
    );
    assert_eq!(
        identifiers_of(&pool, &target).await,
        vec![ident("isbn", "999")],
        "and no record of what it added, so the survivor keeps the tuple"
    );
}
