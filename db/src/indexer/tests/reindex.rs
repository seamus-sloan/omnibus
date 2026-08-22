//! `reindex` end to end: preserving an existing index when the scan fails,
//! the shared-path cross-format deletion guard, the returned stats, and the
//! scan-root display name.

use crate::books::list_books;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, make_test_dir, CoversTempDir};

use super::super::*;
use super::{now_secs, seed_ebook_at};

#[tokio::test]
async fn reindex_propagates_scan_error_without_clearing_existing_index() {
    // Preserve-on-failure invariant: a fatal scan error (here, a
    // library path that doesn't exist on disk) must return Err and
    // leave the existing index completely untouched — we'd rather
    // serve stale-but-good data than wipe the table. Seed one book,
    // point `reindex` at a non-existent directory, and assert both the
    // Err and that the original row survives.
    let _covers = CoversTempDir::new("reindex-preserve");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // A path under a temp dir that we never create — `stat_ebook_library`
    // reports `path not found`, which `reindex` turns into a bail!.
    let missing_path = std::env::temp_dir().join(format!("omnibus-nonexistent-{}", now_secs()));
    assert!(
        !missing_path.exists(),
        "test precondition: the library path must not exist"
    );
    let missing = missing_path.to_string_lossy().into_owned();

    replace_books(
        &pool,
        &missing,
        vec![indexed(
            "a.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    assert_eq!(
        list_books(&pool, &missing).await.unwrap().len(),
        1,
        "test precondition: the book row must be seeded"
    );

    let result = reindex(&pool, &missing).await;
    assert!(
        result.is_err(),
        "reindex must surface the fatal scan error as Err"
    );

    // The pre-existing index is intact — reindex never touched the DB.
    let after = list_books(&pool, &missing).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "a failed scan must preserve the existing index"
    );
    assert_eq!(after[0].title.as_deref(), Some("Dracula"));
}

// ---------- #328: shared-path cross-format deletion guard ----------

/// Seed one `IndexedAudiobook` row into the DB at `library_path`.
/// Bypasses the full audiobook indexer (no real .m4b file needed) so
/// the cross-format-deletion regression tests can focus on the
/// reindex read-side filter, not on parsing real audio files.
async fn seed_audiobook_row(pool: &SqlitePool, library_path: &str, group_path: &str) {
    let plan = crate::sync::AudiobookSyncPlan {
        new_books: vec![crate::audiobook::IndexedAudiobook {
            scan_key: group_path.to_string(),
            group_path: group_path.to_string(),
            format: "M4B".to_string(),
            title: "AudioTitle".to_string(),
            creator_name: None,
            cover: None,
            accent: None,
            parts: vec![],
            chapters: vec![],
            total_size_bytes: 100,
            max_mtime_epoch: 100,
            description: None,
            error: None,
        }],
        ..Default::default()
    };
    crate::sync::sync_audiobooks(pool, library_path, plan)
        .await
        .unwrap();
}

#[tokio::test]
async fn reindex_audiobooks_does_not_delete_ebook_rows_when_libraries_share_a_path() {
    // #328: when ebook_library_path == audiobook_library_path, an
    // audiobook reindex must not classify every EPUB row as Removed
    // and delete it. The fix scopes the diff's "currently indexed"
    // view to the audiobook formats only.
    let _covers = CoversTempDir::new("reindex-audio-shared");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // A real shared dir holding a real EPUB but no audio files — the exact
    // configuration the bug reproduces on. The EPUB file keeps the walk
    // non-empty so the #819 guard trusts the (audio-empty) enumeration and
    // still runs the removal pass for the absent M4B.
    let shared = crate::test_support::make_test_dir("reindex-audio-shared-lib");
    let shared_path = shared.to_string_lossy().into_owned();
    std::fs::write(shared.join("dracula.epub"), b"not a zip").unwrap();

    // Pre-seed one EPUB row and one audiobook row at the same
    // library_path — the exact configuration the bug reproduces on.
    replace_books(
        &pool,
        &shared_path,
        vec![indexed(
            "dracula.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    seed_audiobook_row(&pool, &shared_path, "audio/AudioTitle.m4b").await;
    assert_eq!(
        list_books(&pool, &shared_path).await.unwrap().len(),
        2,
        "test precondition: two books (EPUB + M4B) seeded at the shared path"
    );

    // Run the audiobook reindex. No .m4b files are on disk, but the EPUB
    // keeps the enumeration non-empty (trustworthy), so the audiobook row is
    // classified Removed; the EPUB row survives because its format is
    // outside the audiobook allow-list.
    reindex_audiobooks(&pool, &shared_path).await.unwrap();

    let after = list_books(&pool, &shared_path).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "EPUB row must survive an audiobook-only reindex"
    );
    assert_eq!(after[0].title.as_deref(), Some("Dracula"));
    assert!(
        after[0].formats.iter().any(|f| f == "EPUB"),
        "the surviving row must be the EPUB, not a stray audiobook"
    );

    let _ = std::fs::remove_dir_all(&shared);
}

#[tokio::test]
async fn reindex_ebooks_does_not_delete_audiobook_rows_when_libraries_share_a_path() {
    // #328 inverse: an ebook reindex against a shared path must not
    // delete audiobook rows for the same symmetric reason.
    let _covers = CoversTempDir::new("reindex-ebook-shared");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let shared = crate::test_support::make_test_dir("reindex-ebook-shared-lib");
    let shared_path = shared.to_string_lossy().into_owned();
    // A real (non-epub) audio file keeps the ebook walk non-empty so the
    // #819 guard trusts the (epub-empty) enumeration and still removes the
    // absent EPUB row.
    std::fs::write(shared.join("AudioTitle.m4b"), b"not audio").unwrap();

    replace_books(
        &pool,
        &shared_path,
        vec![indexed(
            "dracula.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    seed_audiobook_row(&pool, &shared_path, "audio/AudioTitle.m4b").await;
    assert_eq!(
        list_books(&pool, &shared_path).await.unwrap().len(),
        2,
        "test precondition: two books (EPUB + M4B) seeded at the shared path"
    );

    // Run the ebook reindex. No .epub files are on disk, but the .m4b keeps
    // the enumeration non-empty (trustworthy), so the EPUB row is classified
    // Removed; the M4B row survives because its format is outside the ebook
    // allow-list.
    reindex(&pool, &shared_path).await.unwrap();

    let after = list_books(&pool, &shared_path).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "audiobook row must survive an ebook-only reindex"
    );
    assert_eq!(after[0].title.as_deref(), Some("AudioTitle"));
    assert!(
        after[0].formats.iter().any(|f| f == "M4B"),
        "the surviving row must be the audiobook, not a stray ebook"
    );

    let _ = std::fs::remove_dir_all(&shared);
}

// ---------- #1057: ReindexStats plumbed off a real scan ----------

/// `reindex` returns the ghost count and pre-scan file-backed total it
/// measured, not just `()` — the tally the worker projects into a
/// [`omnibus_shared::GhostFilesWarning`] on the wire.
#[tokio::test]
async fn reindex_returns_stats_with_the_removed_count_and_file_backed_total() {
    let _covers = CoversTempDir::new("reindex-stats");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-stats-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "a.epub", "Dracula").await;
    seed_ebook_at(&pool, &lib_path, "b.epub", "Frankenstein").await;
    seed_ebook_at(&pool, &lib_path, "c.epub", "Carmilla").await;
    std::fs::remove_file(lib.join("b.epub")).unwrap();

    let stats = reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(stats.removed, 1, "exactly one book's file went missing");
    assert_eq!(
        stats.file_backed_total, 3,
        "the pre-scan file-backed total is measured before this scan's removal"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// `root_display_name` keeps the directory basename through trailing
/// separators (`Path::file_name` ignores them — see its std doc example
/// `/usr/bin/` → `bin`); only the degenerate roots `/` and a `..`-ending
/// path yield no name, and `display_item` then degrades to the bare
/// relative path rather than emitting a leading slash.
#[test]
fn root_display_name_survives_trailing_separators_and_degrades_for_rootless_paths() {
    assert_eq!(root_display_name("/mnt/media/books"), "books");
    assert_eq!(root_display_name("/mnt/media/books/"), "books");
    assert_eq!(root_display_name("/"), "");
    assert_eq!(
        display_item(&root_display_name("/mnt/media/books/"), "Author/Title.epub"),
        "books/Author/Title.epub"
    );
    assert_eq!(
        display_item(&root_display_name("/"), "Author/Title.epub"),
        "Author/Title.epub"
    );
}
