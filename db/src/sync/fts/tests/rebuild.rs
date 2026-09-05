//! `rebuild_all_fts`: reconstructing a corrupted or fully-emptied index,
//! repopulating genres from the override JSON, and its batched
//! `INSERT ... SELECT` matching the per-book `upsert_fts` row for row.

use sqlx::Row;

use super::super::*;
use super::{assert_fts_invariant, fts_genre_hits, fts_isbn_hits, indexed_with_isbn};
use crate::pool::init_db;
use crate::sync::{sync_books, SyncPlan};
use crate::test_support::{count_rows, indexed, CoversTempDir};

/// One `books_fts` row's plain-text columns, ordered by `rowid`. Used to
/// diff the batched rebuild's output against the per-book upsert path.
type FtsRowSnapshot = (i64, String, String, String, String, String, String, String);

/// Snapshot every `books_fts` row (all eight columns) ordered by `rowid`,
/// so two populations of the same `books` table can be compared for exact
/// content equality regardless of which code path produced them.
async fn snapshot_fts_rows(pool: &sqlx::SqlitePool) -> Vec<FtsRowSnapshot> {
    sqlx::query(
        "SELECT rowid, title, authors, series, tags, description, isbn, genres
         FROM books_fts ORDER BY rowid",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| {
        (
            r.get::<i64, _>("rowid"),
            r.get::<String, _>("title"),
            r.get::<String, _>("authors"),
            r.get::<String, _>("series"),
            r.get::<String, _>("tags"),
            r.get::<String, _>("description"),
            r.get::<String, _>("isbn"),
            r.get::<String, _>("genres"),
        )
    })
    .collect()
}

/// Seed a varied fixture exercising every branch of the FTS projection:
/// multiple authors, a series, multiple tags, an ISBN, and a book with
/// none of the optional taxonomy links — so a diff between the per-book
/// and batched projections would catch a mismatch in any one column.
async fn seed_varied_fts_fixture(pool: &sqlx::SqlitePool) {
    sync_books(
        pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                indexed(
                    "multi.epub",
                    Some("Multi Author Saga"),
                    &["Ada Lovelace", "Grace Hopper"],
                    &["fiction", "classic"],
                    Some(("Saga", "1")),
                    None,
                ),
                indexed_with_isbn("solo.epub", "Solo Work", "Bram Stoker", "9781111111111"),
                indexed("bare.epub", Some("Bare Book"), &[], &[], None, None),
                indexed(
                    "tags.epub",
                    Some("Tagged Only"),
                    &["Cy"],
                    &["nonfiction", "essay", "history"],
                    None,
                    None,
                ),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn rebuild_all_fts_reconstructs_index_after_corruption() {
    let _covers = CoversTempDir::new("fts_rebuild_all");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                indexed_with_isbn("a.epub", "Alpha", "Ann", "9782222222222"),
                indexed("b.epub", Some("Beta"), &["Bob"], &[], None, None),
                indexed("c.epub", Some("Gamma"), &["Cy"], &[], None, None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Corrupt the index two ways: drop one real row (creating a missing
    // entry) and insert an orphan row for a non-existent book id.
    sqlx::query("DELETE FROM books_fts WHERE rowid = (SELECT id FROM books WHERE title = 'Alpha')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn, genres) \
                 VALUES (999999, 'orphan', '', '', '', '', '', '')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Sanity: the index is now structurally drifted — a real book lacks
    // its FTS row, and an orphan FTS row exists. (The two cancel out in a
    // raw row count, so check the anti-joins, not the totals.)
    let missing_before = count_rows(
        &pool,
        "SELECT COUNT(*) FROM books b
          WHERE NOT EXISTS (SELECT 1 FROM books_fts f WHERE f.rowid = b.id)",
    )
    .await;
    let orphans_before = count_rows(
        &pool,
        "SELECT COUNT(*) FROM books_fts f
          WHERE NOT EXISTS (SELECT 1 FROM books b WHERE b.id = f.rowid)",
    )
    .await;
    assert_eq!(
        missing_before, 1,
        "the dropped book should now lack an FTS row"
    );
    assert_eq!(orphans_before, 1, "the planted row should be an orphan");
    assert_eq!(fts_isbn_hits(&pool, "9782222222222").await, 0);

    rebuild_all_fts(&pool).await.unwrap();

    // Invariant restored, orphan swept, the dropped ISBN searchable again.
    assert_fts_invariant(&pool).await;
    assert_eq!(fts_isbn_hits(&pool, "9782222222222").await, 1);
    let orphan_after =
        count_rows(&pool, "SELECT COUNT(*) FROM books_fts WHERE rowid = 999999").await;
    assert_eq!(orphan_after, 0, "orphan row must be swept by the rebuild");
}

#[tokio::test]
async fn rebuild_all_fts_repopulates_genres_from_the_override_json() {
    // `genres` is the one indexed column with no canonical table behind it,
    // so the admin rebuild has to read `metadata_overrides` to restore it —
    // reconstructing from `books` and its links alone would drop it silently
    // while still leaving the row-count parity check green.
    let _covers = CoversTempDir::new("fts_rebuild_genres");
    let pool = init_db("sqlite::memory:").await.unwrap();
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed("a.epub", Some("Alpha"), &["Ann"], &[], None, None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::uuid_by_scan_key(&pool, "a.epub").await;
    crate::metadata_overrides::merge_metadata_overrides(
        &pool,
        &uuid,
        &omnibus_shared::MetadataOverrides {
            genres: Some(vec!["Horror".into(), "Gothic".into()]),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(fts_genre_hits(&pool, "Horror").await, 1);

    rebuild_all_fts(&pool).await.unwrap();

    assert_fts_invariant(&pool).await;
    assert_eq!(
        fts_genre_hits(&pool, "Horror").await,
        1,
        "the rebuild must restore genres, not blank the column"
    );
    assert_eq!(fts_genre_hits(&pool, "Gothic").await, 1);
}

#[tokio::test]
async fn rebuild_all_fts_batched_insert_matches_per_book_upsert_fts_row_for_row() {
    // Regression for the batching change (issue #1166): `rebuild_all_fts`
    // used to loop `upsert_fts` once per book (one DELETE + one INSERT
    // each); it now does a single whole-table `DELETE` + `INSERT ...
    // SELECT`. The two share the `FTS_SELECT_FROM_BOOKS` projection, but
    // this test diffs their actual output row-for-row so a future edit to
    // either query can't silently drift the two apart.
    let _covers = CoversTempDir::new("fts_diff_batched_vs_per_book");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_varied_fts_fixture(&pool).await;

    // `sync_books`'s New path already populated `books_fts` by calling
    // `upsert_fts` once per inserted book — capture that as the per-book
    // reference before touching the table.
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM books ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(ids.len(), 4, "fixture should seed four distinct books");
    let via_per_book_upsert = snapshot_fts_rows(&pool).await;
    assert_eq!(via_per_book_upsert.len(), 4);

    // Wipe the index and repopulate it through the new batched rebuild.
    sqlx::query("DELETE FROM books_fts")
        .execute(&pool)
        .await
        .unwrap();
    rebuild_all_fts(&pool).await.unwrap();
    let via_batched_rebuild = snapshot_fts_rows(&pool).await;

    assert_eq!(
        via_per_book_upsert, via_batched_rebuild,
        "the batched whole-table INSERT must produce byte-identical rows \
         to the per-book upsert_fts path for every book"
    );
}

#[tokio::test]
async fn rebuild_all_fts_fully_repopulates_multi_book_library_after_total_index_loss() {
    // Acceptance criterion: a multi-book fixture's FTS table is fully
    // repopulated after the batched rebuild, even from a completely empty
    // index (not just a partially-drifted one, covered separately by
    // `rebuild_all_fts_reconstructs_index_after_corruption`).
    let _covers = CoversTempDir::new("fts_rebuild_full_repopulation");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_varied_fts_fixture(&pool).await;

    sqlx::query("DELETE FROM books_fts")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books_fts").await, 0);

    rebuild_all_fts(&pool).await.unwrap();

    assert_fts_invariant(&pool).await;
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 4);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books_fts").await, 4);
    // Spot-check every branch of the projection actually landed in the
    // rebuilt index: multi-author group_concat, ISBN, and multi-tag.
    let multi_author_hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE authors MATCH 'Lovelace'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(multi_author_hits, 1);
    assert_eq!(fts_isbn_hits(&pool, "9781111111111").await, 1);
    let tag_hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE tags MATCH 'history'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tag_hits, 1);
}
