//! `attach_derived_percent` / `derive_epub_percent`: deriving a whole-book
//! percent from a stored CFI against the source EPUB on disk, and the cases
//! that decline to derive one.

use omnibus_shared::{EbookMetadata, ProgressUpdate};
use sqlx::SqlitePool;

use crate::{init_db, replace_books};

use super::super::*;
use super::{seed, seed_user};

// ── derived-percent attachment (#1864) ──────────────────────────────

/// One-paragraph chapter used twice so the whole-book percent at the start
/// of chapter 2 is exactly 50 (identical visible-text counts per chapter).
const PERCENT_CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C</title></head>
<body>
  <p>First sentence here. Second sentence follows.</p>
</body>
</html>"#;

/// Seed one book whose two-chapter EPUB really exists on disk in a per-test
/// library dir, plus a user; returns `(pool, user_id, book_uuid)`.
async fn seed_epub_on_disk(tag: &str) -> (SqlitePool, i64, String) {
    let dir = crate::test_support::make_test_dir(&format!("derive_percent_{tag}"));
    std::fs::write(
        dir.join("book.epub"),
        crate::test_support::build_test_epub(&[
            ("c1.xhtml", PERCENT_CHAPTER),
            ("c2.xhtml", PERCENT_CHAPTER),
        ]),
    )
    .unwrap();
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed_named_file(&pool, dir.to_str().unwrap(), "Book", "book.epub").await;
    let user = seed_user(&pool, "alice").await;
    (pool, user, uuid)
}

/// Like [`seed`] but with an explicit on-disk filename, so `book_file_path`
/// resolves to a file the test actually wrote.
async fn seed_named_file(
    pool: &SqlitePool,
    library: &str,
    title: &str,
    filename: &str,
) -> (i64, String) {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: filename.to_string(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .expect("seed book");
    let books = crate::list_books(pool, library).await.unwrap();
    let book = books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap();
    (book.id, book.unique_identifier.clone().unwrap())
}

fn cfi_update(uuid: &str, cfi: &str, client_updated_at: i64) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: Some(cfi.to_string()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: Some(client_updated_at),
    }
}

#[tokio::test]
async fn attach_derived_percent_sets_percent_only_when_clock_matches() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let saved = upsert_progress(
        &pool,
        user,
        &cfi_update(&uuid, "epubcfi(/6/4!/4/2/1:0)", 1_000),
    )
    .await
    .unwrap();
    assert_eq!(saved.client_updated_at, 1_000);

    // Stale expectation: the row's event time is 1_000, not 999.
    let stale = attach_derived_percent(&pool, user, &uuid, 43, 999)
        .await
        .unwrap();
    assert!(!stale, "a mismatched clock must not attach");

    let attached = attach_derived_percent(&pool, user, &uuid, 43, 1_000)
        .await
        .unwrap();
    assert!(attached);
    let row = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.progress_percent, Some(43));
    // Clock-neutral: neither freshness clock moved.
    assert_eq!(row.client_updated_at, saved.client_updated_at);
    assert_eq!(row.updated_at, saved.updated_at);

    // A percent already present is never overwritten by a derivation.
    let second = attach_derived_percent(&pool, user, &uuid, 77, 1_000)
        .await
        .unwrap();
    assert!(!second);
    let row = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.progress_percent, Some(43));
}

#[tokio::test]
async fn derive_epub_percent_attaches_visible_text_percent_from_source_epub() {
    let (pool, user, uuid) = seed_epub_on_disk("happy").await;
    // Spine index 1 (`/6/4`), first text node, offset 0 — the first visible
    // character of chapter 2 of two identical chapters: exactly 50%.
    let cfi = "epubcfi(/6/4!/4/2/1:0)";
    let saved = upsert_progress(&pool, user, &cfi_update(&uuid, cfi, 1_000))
        .await
        .unwrap();
    assert_eq!(saved.progress_percent, None);

    let derived = derive_epub_percent(&pool, user, &uuid, cfi, saved.client_updated_at)
        .await
        .unwrap();
    assert!(derived);
    let row = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.progress_percent, Some(50));
    assert_eq!(row.client_updated_at, saved.client_updated_at);
}

#[tokio::test]
async fn derive_epub_percent_returns_false_when_book_has_no_epub_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // A ghosted book: `books` row only, no `book_files` — the shape a
    // removed file leaves behind (F2).
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib2', 'lib2')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         SELECT 'ghost-uuid', 'g.epub', id, '/lib2/g', 'Ghost', 'ghost'
           FROM scan_roots WHERE path = '/lib2'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let derived = derive_epub_percent(&pool, user, "ghost-uuid", "epubcfi(/6/2!/4/2/1:0)", 1_000)
        .await
        .unwrap();
    assert!(!derived, "a fileless book must degrade to no derivation");
}

#[tokio::test]
async fn derive_epub_percent_returns_false_for_an_unparseable_cfi() {
    let (pool, user, uuid) = seed_epub_on_disk("unparseable").await;
    // A comic-page anchor reuses the CFI slot but is not a CFI; the parse
    // refuses it and nothing attaches.
    let derived = derive_epub_percent(&pool, user, &uuid, "comic-page:3", 1_000)
        .await
        .unwrap();
    assert!(!derived);
}

#[tokio::test]
async fn derive_epub_percent_agrees_with_stored_spine_stats() {
    // Same two-identical-chapter book as the full-walk test, but with the
    // 0071 structure extracted first: the stats fast-path must land the
    // same value (exactly 50 at the start of chapter 2), and the row's
    // clocks must stay untouched.
    let dir = crate::test_support::make_test_dir("derive_percent_stats");
    std::fs::write(
        dir.join("book.epub"),
        crate::test_support::build_test_epub(&[
            ("c1.xhtml", PERCENT_CHAPTER),
            ("c2.xhtml", PERCENT_CHAPTER),
        ]),
    )
    .unwrap();
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed_named_file(&pool, dir.to_str().unwrap(), "Book", "book.epub").await;
    let user = seed_user(&pool, "alice").await;
    crate::indexer::backfill_epub_structure(&pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();
    let file_id: i64 = sqlx::query_scalar("SELECT id FROM book_files LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !crate::epub_structure::get_spine_stats(&pool, file_id)
            .await
            .unwrap()
            .is_empty(),
        "precondition: stats extracted so the fast path is the one exercised"
    );

    let cfi = "epubcfi(/6/4!/4/2/1:0)";
    let saved = upsert_progress(&pool, user, &cfi_update(&uuid, cfi, 1_000))
        .await
        .unwrap();
    let derived = derive_epub_percent(&pool, user, &uuid, cfi, saved.client_updated_at)
        .await
        .unwrap();
    assert!(derived);
    let row = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.progress_percent, Some(50));
    assert_eq!(row.client_updated_at, saved.client_updated_at);
    assert_eq!(row.updated_at, saved.updated_at);
}
