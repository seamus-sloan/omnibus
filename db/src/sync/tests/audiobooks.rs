//! `sync_audiobooks` end-to-end: accent-color round-trip and validation,
//! the Removed bucket above the bind cap and its fileless/returning-group
//! relink, and the parts + chapters rows written for multi-part, empty and
//! single-chapter audiobooks.

use super::super::*;
use crate::books::{get_book, list_books};
use crate::pool::init_db;
use crate::test_support::{indexed_audiobook, CoversTempDir};

#[tokio::test]
async fn sync_audiobooks_round_trips_accent_color() {
    let _covers = CoversTempDir::new("ab_accent_round_trip");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut with_accent = indexed_audiobook("Author/Book.m4b", "Accented Book", Some("Author"));
    with_accent.accent = Some("oklch(0.660 0.130 245.0)".into());

    let mut no_accent = indexed_audiobook("Author/Plain.m4b", "Plain Book", Some("Author"));
    no_accent.accent = None;

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![with_accent, no_accent],
            ..Default::default()
        },
    )
    .await
    .expect("sync should succeed");

    let books = list_books(&pool, "/lib").await.unwrap();
    let accented = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Accented Book"))
        .unwrap();
    let plain = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Plain Book"))
        .unwrap();
    assert_eq!(accented.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
    assert_eq!(plain.accent, None);

    let detail = get_book(&pool, accented.id).await.unwrap().unwrap();
    assert_eq!(detail.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
}

#[tokio::test]
async fn sync_audiobooks_updates_accent_color_on_changed() {
    let _covers = CoversTempDir::new("ab_accent_update");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let book = indexed_audiobook("Author/Book.m4b", "Book", Some("Author"));
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books[0].accent, None, "initially no accent");

    let mut updated = indexed_audiobook("Author/Book.m4b", "Book", Some("Author"));
    updated.accent = Some("oklch(0.700 0.100 180.0)".into());

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            changed_books: vec![updated],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        books[0].accent.as_deref(),
        Some("oklch(0.700 0.100 180.0)"),
        "accent should be set after update"
    );
}

#[tokio::test]
async fn sync_audiobooks_drops_unsafe_accent_color() {
    let _covers = CoversTempDir::new("ab_accent_unsafe");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Shady.m4b", "Shady", Some("Author"));
    book.accent = Some("red; background: url(x)".into());

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        books[0].accent, None,
        "unsafe accent must be sanitized to NULL"
    );
}

/// Removing a single audiobook diff bucket that exceeds SQLite's 999-bind
/// parameter cap must succeed: `sync_audiobooks_removed` has to chunk the
/// `WHERE uuid IN (?, ?, ...)` list (mirroring `sync_removed` in books.rs).
/// Un-chunked, a single 1000-uuid removal would bind library_id + 1000 uuids
/// and fail at runtime with "too many SQL variables".
#[tokio::test]
async fn sync_audiobooks_with_removed_above_bind_cap_succeeds() {
    let _covers = CoversTempDir::new("ab_remove_chunk");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // 1000 audiobooks: pushes the un-chunked IN(?, ?, ...) over SQLite's
    // 999-bind cap (1 library_id + 1000 uuids = 1001 binds). 1000 also
    // forces the chunked path through two chunks (500 + 500).
    const N: usize = 1000;
    let new_books: Vec<_> = (0..N)
        .map(|i| {
            indexed_audiobook(
                &format!("Author/Book{i:04}.m4b"),
                &format!("Book {i}"),
                Some("Author"),
            )
        })
        .collect();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books,
            ..Default::default()
        },
    )
    .await
    .expect("initial sync of 1000 audiobooks should succeed");
    assert_eq!(list_books(&pool, "/lib").await.unwrap().len(), N);
    // Identity is minted (F2) — collect the durable uuids to remove from the DB.
    let all_uuids: Vec<String> = sqlx::query_scalar("SELECT uuid FROM books")
        .fetch_all(&pool)
        .await
        .unwrap();

    // Wholesale remove all 1000 in a single plan — this is the scenario
    // the issue calls out (massive library disappearing in a single scan).
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            removed_uuids: all_uuids,
            ..Default::default()
        },
    )
    .await
    .expect("wholesale removal of >500 audiobooks must not exceed bind cap");

    assert!(
        list_books(&pool, "/lib").await.unwrap().is_empty(),
        "every audiobook is hidden from the grid (fileless) after wholesale removal"
    );
    let books_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        books_total, fts_count,
        "every fileless audiobook keeps its books + FTS row"
    );
}

/// F2 for audiobooks: removing a group makes its book fileless (hidden from the
/// grid, `books` row + durable uuid retained); when the same group returns via
/// the New bucket it re-attaches to that row, preserving the uuid. Exercises the
/// batched scan_key pre-fetch map in `sync_audiobooks_new` driving the
/// rewrite-in-place branch instead of a per-book `SELECT`.
#[tokio::test]
async fn sync_audiobooks_removed_group_goes_fileless_then_returning_group_relinks_same_uuid() {
    let _covers = CoversTempDir::new("ab_fileless_relink");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook("Author/Book.m4b", "Book", Some("Author"))],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let uuid1 = crate::test_support::uuid_by_scan_key(&pool, "Author/Book.m4b").await;

    // Group gone → fileless: hidden from the grid, row + uuid survive.
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            removed_uuids: vec![uuid1.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        list_books(&pool, "/lib").await.unwrap().is_empty(),
        "fileless audiobook is hidden from the library grid"
    );
    assert_eq!(
        crate::test_support::uuid_by_scan_key(&pool, "Author/Book.m4b").await,
        uuid1,
        "fileless audiobook retains its scan_key and durable uuid"
    );

    // Group returns via New → the batched map resolves the same-scan_key row and
    // rewrites in place, preserving the uuid.
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook("Author/Book.m4b", "Book", Some("Author"))],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let after = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].unique_identifier.as_deref(),
        Some(uuid1.as_str()),
        "returning group relinks to the same uuid via the batched New path"
    );
    // Exactly one books row + one book_files row — the batched pre-fetch must not
    // mint a duplicate for the returning group.
    let books_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    let files_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        books_total, 1,
        "no duplicate books row for the returning group"
    );
    assert_eq!(
        files_total, 1,
        "returning group re-creates exactly one file row"
    );
}

// Audiobook parts + chapters: row contents and edge cases.
#[tokio::test]
async fn sync_audiobooks_writes_all_parts_for_a_five_part_audiobook() {
    let _covers = CoversTempDir::new("ab_bulk_parts");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Book", "Big Book", Some("Author"));
    book.parts = (0..5)
        .map(|i| crate::audiobook::AudiobookPart {
            ordinal: i,
            filename: format!("Author/Book/part{i:02}.m4b"),
            size_bytes: 1000 + i,
            mtime_epoch: 100 + i,
            duration_seconds: 60.0 * (i + 1) as f64,
        })
        .collect();
    // No embedded chapters → synthetic-fallback path writes one chapter per part.
    book.chapters = vec![];

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let rows: Vec<(i64, String, i64, i64, f64)> = sqlx::query_as(
        "SELECT ordinal, filename, size_bytes, mtime_epoch, duration_seconds \
         FROM book_file_parts ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 5);
    for (i, (ordinal, filename, size_bytes, mtime_epoch, duration_seconds)) in
        rows.iter().enumerate()
    {
        let i = i as i64;
        assert_eq!(*ordinal, i);
        assert_eq!(filename, &format!("Author/Book/part{i:02}.m4b"));
        assert_eq!(*size_bytes, 1000 + i);
        assert_eq!(*mtime_epoch, 100 + i);
        assert_eq!(*duration_seconds, 60.0 * (i + 1) as f64);
    }
}

#[tokio::test]
async fn sync_audiobooks_writes_all_fifty_chapters_for_a_five_part_audiobook() {
    let _covers = CoversTempDir::new("ab_bulk_chapters");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Big.m4b", "Big Book", Some("Author"));
    // One 60-minute part — chapter timestamps are absolute from book start.
    book.parts = vec![crate::audiobook::AudiobookPart {
        ordinal: 0,
        filename: "Author/Big.m4b".into(),
        size_bytes: 99_999,
        mtime_epoch: 500,
        duration_seconds: 3600.0,
    }];
    // 50 sequential chapters, each 60 s, with `end_ms == 0` so the gap-fill
    // branch derives the duration from the next chapter's start.
    book.chapters = (0..50)
        .map(|i| crate::audiobook::RawChapter {
            title: format!("Chapter {i}"),
            start_ms: (i as u64) * 60_000,
            end_ms: 0,
        })
        .collect();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let chapters: Vec<(i64, String, f64, f64)> = sqlx::query_as(
        "SELECT ordinal, title, start_seconds, duration_seconds \
         FROM file_chapters ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(chapters.len(), 50);
    for (i, (ordinal, title, start_seconds, duration_seconds)) in chapters.iter().enumerate() {
        let i = i as i64;
        assert_eq!(*ordinal, i);
        assert_eq!(title, &format!("Chapter {i}"));
        assert_eq!(*start_seconds, (i as f64) * 60.0);
        // Chapters 0..=48 fall to the gap-fill branch (next chapter's start
        // minus this chapter's start = 60 s). Chapter 49 has no next, so it
        // falls back to `total_duration - start` = 3600 - 2940 = 660 s.
        let expected = if i < 49 { 60.0 } else { 660.0 };
        assert_eq!(*duration_seconds, expected, "ordinal {i} duration mismatch");
    }
}

#[tokio::test]
async fn sync_audiobooks_writes_zero_parts_and_synthesized_chapter_for_empty_parts_edge_case() {
    let _covers = CoversTempDir::new("ab_bulk_empty");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Only.m4b", "Only", Some("Author"));
    book.parts = vec![];
    book.chapters = vec![];

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let parts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_file_parts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(parts_count, 0, "empty `parts` writes no `book_file_parts`");

    let chapters_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_chapters")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        chapters_count, 0,
        "no parts and no chapters → synthetic fallback is also empty"
    );
}

#[tokio::test]
async fn sync_audiobooks_writes_one_chapter_when_single_chapter_provided() {
    let _covers = CoversTempDir::new("ab_bulk_single_chap");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/One.m4b", "Solo", Some("Author"));
    book.parts = vec![crate::audiobook::AudiobookPart {
        ordinal: 0,
        filename: "Author/One.m4b".into(),
        size_bytes: 2048,
        mtime_epoch: 42,
        duration_seconds: 120.0,
    }];
    book.chapters = vec![crate::audiobook::RawChapter {
        title: "Only".into(),
        start_ms: 0,
        end_ms: 0,
    }];

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let rows: Vec<(i64, String, f64, f64)> = sqlx::query_as(
        "SELECT ordinal, title, start_seconds, duration_seconds \
         FROM file_chapters ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 0);
    assert_eq!(rows[0].1, "Only");
    assert_eq!(rows[0].2, 0.0);
    // Single chapter with end_ms == 0 → duration falls back to total_duration - start = 120 s.
    assert_eq!(rows[0].3, 120.0);
}
