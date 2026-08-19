//! Acceptance tests for the `books_fts` choke-point: the no-orphan invariant
//! after every public write path, the cross-format attach gap regression,
//! `rebuild_all_fts` reconstructing a corrupted or fully-emptied index, and its
//! batched `INSERT ... SELECT` matching the per-book `upsert_fts` path.

use omnibus_shared::{Contributor, EbookMetadata, Identifier};
use sqlx::Row;

use super::*;
use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{count_rows, indexed, indexed_audiobook, CoversTempDir};

/// Build an `IndexedBook` with a single ISBN identifier so attach/union
/// paths have an ISBN to carry into the target's FTS row.
fn indexed_with_isbn(filename: &str, title: &str, author: &str, isbn: &str) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some(title.into()),
            creators: vec![Contributor {
                name: author.into(),
                ..Default::default()
            }],
            identifiers: vec![Identifier {
                value: isbn.into(),
                scheme: Some("ISBN".into()),
            }],
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// Assert the standalone FTS twin is in lock-step with `books`: every
/// `books` row has exactly one `books_fts` row, and no `books_fts` row is
/// orphaned (no backing `books` row).
async fn assert_fts_invariant(pool: &sqlx::SqlitePool) {
    // Every `books` row — file-backed or fileless (F2) — keeps exactly one
    // `books_fts` row, so a fileless book stays searchable; the grid/facets
    // hide it via their own `EXISTS book_files` filter.
    let books = count_rows(pool, "SELECT COUNT(*) FROM books").await;
    let fts = count_rows(pool, "SELECT COUNT(*) FROM books_fts").await;
    assert_eq!(books, fts, "books_fts row count must equal books count");

    let missing = count_rows(
        pool,
        "SELECT COUNT(*) FROM books b
          WHERE NOT EXISTS (SELECT 1 FROM books_fts f WHERE f.rowid = b.id)",
    )
    .await;
    assert_eq!(missing, 0, "every book must have a books_fts row");

    let orphans = count_rows(
        pool,
        "SELECT COUNT(*) FROM books_fts f
          WHERE NOT EXISTS (SELECT 1 FROM books b WHERE b.id = f.rowid)",
    )
    .await;
    assert_eq!(orphans, 0, "no books_fts row may point at a deleted book");
}

/// Count `books_fts` rows whose `isbn` column MATCHes the term.
async fn fts_isbn_hits(pool: &sqlx::SqlitePool, isbn: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM books_fts WHERE isbn MATCH ?")
        .bind(isbn)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// One `books_fts` row's plain-text columns, ordered by `rowid`. Used to
/// diff the batched rebuild's output against the per-book upsert path.
type FtsRowSnapshot = (i64, String, String, String, String, String, String);

/// Snapshot every `books_fts` row (all seven columns) ordered by `rowid`,
/// so two populations of the same `books` table can be compared for exact
/// content equality regardless of which code path produced them.
async fn snapshot_fts_rows(pool: &sqlx::SqlitePool) -> Vec<FtsRowSnapshot> {
    sqlx::query(
        "SELECT rowid, title, authors, series, tags, description, isbn
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
async fn fts_invariant_holds_after_sync_books_new_changed_and_removed() {
    let _covers = CoversTempDir::new("fts_invariant_ebooks");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // New.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                indexed("a.epub", Some("Alpha"), &["Ann"], &["sci-fi"], None, None),
                indexed("b.epub", Some("Beta"), &["Bob"], &[], None, None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_fts_invariant(&pool).await;

    // Changed (same filename, new title) — preserves id, refreshes FTS.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            changed_books: vec![indexed(
                "a.epub",
                Some("Alpha Prime"),
                &["Ann"],
                &["sci-fi"],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Removed → fileless (F2). Identity is minted, so resolve b.epub's uuid
    // by scan_key, then assert it is retained as a fileless book that keeps
    // its FTS row (2 books total, 1 file-backed).
    let b_uuid = crate::test_support::uuid_by_scan_key(&pool, "b.epub").await;
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![b_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        2,
        "removed book retained as a fileless book"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books b
              WHERE EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)"
        )
        .await,
        1,
        "only the surviving book is file-backed"
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn fts_invariant_holds_after_replace_books() {
    let _covers = CoversTempDir::new("fts_invariant_replace");
    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::sync::replace_books(
        &pool,
        "/lib",
        vec![
            indexed("x.epub", Some("Ex"), &["Xi"], &[], None, None),
            indexed("y.epub", Some("Why"), &["Yi"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Replace again with one fewer book — the dropped book's FTS row must go,
    // but the book is retained as a fileless book (F2): 2 books total, 1
    // file-backed (and therefore 1 FTS row).
    crate::sync::replace_books(
        &pool,
        "/lib",
        vec![indexed("x.epub", Some("Ex"), &["Xi"], &[], None, None)],
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books b
              WHERE EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)"
        )
        .await,
        1
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn fts_invariant_holds_after_sync_audiobooks_new_changed_and_removed() {
    let _covers = CoversTempDir::new("fts_invariant_audiobooks");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let ab = indexed_audiobook("Stoker/Dracula", "Dracula", Some("Bram Stoker"));
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_fts_invariant(&pool).await;
    // Identity is minted (F2) — read the durable uuid back by scan_key.
    let uuid = crate::test_support::uuid_by_scan_key(&pool, "Stoker/Dracula").await;

    // Changed — matched by scan_key (the group path), identity preserved.
    let changed = indexed_audiobook(
        "Stoker/Dracula",
        "Dracula (Unabridged)",
        Some("Bram Stoker"),
    );
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            changed_books: vec![changed],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Removed → fileless (F2): the books row is retained, its book_files row
    // is gone, but its links + FTS row stay (it remains searchable).
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            removed_uuids: vec![uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "removed book is retained as a fileless book"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn attaching_second_ebook_format_makes_its_new_isbn_searchable() {
    // Regression for the attach gap: a second format attaching to an
    // existing book unions its identifiers (incl. ISBN), but the pre-fix
    // code never refreshed the target's FTS row — so the ISBN wasn't
    // searchable. This test fails on the old code and passes after the
    // door is called from `attach_ebook_file`.
    let _covers = CoversTempDir::new("fts_attach_ebook_isbn");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Seed an EPUB with no ISBN.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed(
                "Dracula.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(fts_isbn_hits(&pool, "9781111111111").await, 0);

    // Attach a second format (MOBI) for the same work carrying a NEW ISBN.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed_with_isbn(
                "Dracula.mobi",
                "Dracula",
                "Bram Stoker",
                "9781111111111",
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Still one book (the MOBI attached, not a new row).
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2
    );
    // The unioned ISBN is now searchable, and the invariant still holds.
    assert_eq!(fts_isbn_hits(&pool, "9781111111111").await, 1);
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn attaching_audiobook_to_existing_ebook_keeps_single_fts_row() {
    // The audiobook attach path carries no identifiers, so there is no new
    // ISBN to surface — but it must still leave exactly one FTS row for the
    // target (no orphan, no duplicate) after routing through the door.
    let _covers = CoversTempDir::new("fts_attach_audiobook");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed(
                "Dracula.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook(
                "Stoker/Dracula",
                "Dracula",
                Some("Bram Stoker"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2
    );
    assert_fts_invariant(&pool).await;
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
        "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn) \
                 VALUES (999999, 'orphan', '', '', '', '', '')",
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
async fn upsert_and_delete_fts_round_trip_a_single_row() {
    let _covers = CoversTempDir::new("fts_door_unit");
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
    let id: i64 = sqlx::query_scalar("SELECT id FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();

    // delete_fts removes the row; upsert_fts puts it back from canonical data.
    let mut conn = pool.acquire().await.unwrap();
    delete_fts(&mut conn, id).await.unwrap();
    let after_delete: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_delete, 0);

    upsert_fts(&mut conn, id).await.unwrap();
    let after_upsert: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_upsert, 1);
    // upsert is idempotent — a second call doesn't duplicate.
    upsert_fts(&mut conn, id).await.unwrap();
    let after_second: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_second, 1);
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
