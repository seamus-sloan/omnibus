//! Unit tests for the library-scale totals. Every one of these is really a
//! test about **coverage**: each input has a state meaning "not measured yet",
//! and a `SUM` that reads it as zero is how a partly-backfilled library
//! reports a confidently-wrong smaller number.

use super::*;
use crate::init_db;

async fn seed_lib(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One `books` row plus one `book_files` row, so the book is *live*. Returns
/// the `book_files.id` for attaching audio parts.
async fn seed_book(
    pool: &SqlitePool,
    lib_id: i64,
    uuid: &str,
    format: &str,
    (word_count, page_count): (Option<i64>, Option<i64>),
) -> i64 {
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, word_count, page_count)
         VALUES (?, ?, '', ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .bind(word_count)
    .bind(page_count)
    .fetch_one(pool)
    .await
    .unwrap();
    add_file(pool, book_id, format, 0).await
}

async fn book_id_of(pool: &SqlitePool, uuid: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn add_file(pool: &SqlitePool, book_id: i64, format: &str, ordinal: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes)
         VALUES (?, ?, 'f', ?, 0) RETURNING id",
    )
    .bind(book_id)
    .bind(format)
    .bind(ordinal)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A `books` row with no file at all — the ghosted case.
async fn seed_ghost(pool: &SqlitePool, lib_id: i64, uuid: &str, word_count: Option<i64>) {
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, word_count) VALUES (?,?,'',?,?)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .bind(word_count)
    .execute(pool)
    .await
    .unwrap();
}

async fn add_part(pool: &SqlitePool, file_id: i64, ordinal: i64, duration: f64) {
    sqlx::query(
        "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, duration_seconds)
         VALUES (?, ?, 'p', 0, ?)",
    )
    .bind(file_id)
    .bind(ordinal)
    .bind(duration)
    .execute(pool)
    .await
    .unwrap();
}

/// `compute` directly: the cache is a process-wide `static`, so the tests that
/// aren't *about* caching must not go through it.
async fn sized(pool: &SqlitePool) -> LibrarySize {
    compute(pool).await.unwrap()
}

// --- words --------------------------------------------------------------

#[tokio::test]
async fn word_total_reports_its_denominator_rather_than_summing_unmeasured_rows_as_zero() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB", (Some(90_000), None)).await;
    seed_book(&pool, lib, "u-b", "EPUB", (Some(60_000), None)).await;
    // Not yet backfilled: it must not read as a zero-word book, and it must
    // not silently vanish from the denominator either.
    seed_book(&pool, lib, "u-c", "EPUB", (None, None)).await;

    let size = sized(&pool).await;

    assert_eq!(size.books, 3);
    assert_eq!(size.words.total, 150_000);
    assert_eq!(size.words.books, 2, "coverage must exclude the NULL row");
}

#[tokio::test]
async fn an_audio_only_book_contributes_no_words_and_no_word_coverage() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let file = seed_book(&pool, lib, "u-audio", "M4B", (None, None)).await;
    add_part(&pool, file, 0, 3600.0).await;

    let size = sized(&pool).await;

    assert_eq!(size.books, 1);
    assert!(size.words.is_empty());
    // It is still a book in the library, and still has hours.
    assert_eq!(size.listening_seconds.total, 3600);
    assert_eq!(size.listening_seconds.books, 1);
}

// --- pages --------------------------------------------------------------

#[tokio::test]
async fn page_total_resolves_each_book_through_the_shared_length_ladder() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    // A comic's exact image count outranks the word estimate; an EPUB falls
    // through to `word_count / 275`.
    seed_book(&pool, lib, "u-comic", "CBZ", (None, Some(32))).await;
    seed_book(&pool, lib, "u-epub", "EPUB", (Some(275 * 100), None)).await;
    seed_book(&pool, lib, "u-unknown", "M4B", (None, None)).await;

    let size = sized(&pool).await;

    assert_eq!(size.pages.total, 132);
    assert_eq!(size.pages.books, 2);
    assert_eq!(size.books, 3);
}

#[tokio::test]
async fn page_total_prefers_a_print_edition_count_over_the_estimate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB", (Some(275 * 100), None)).await;
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides)
         VALUES ('u-a', json_object('print_pages', 412))",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(sized(&pool).await.pages.total, 412);
}

// --- audiobook hours ----------------------------------------------------

#[tokio::test]
async fn a_multi_part_audiobook_counts_once_and_sums_all_of_its_parts() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let file = seed_book(&pool, lib, "u-audio", "M4B", (None, None)).await;
    for (ordinal, duration) in [(0, 1800.0), (1, 1800.0), (2, 3600.0)] {
        add_part(&pool, file, ordinal, duration).await;
    }

    let size = sized(&pool).await;

    // Three parts is one audiobook, not three — `book_file_parts` is one row
    // per file, and counting rows reports a rip as a shelf.
    assert_eq!(size.listening_seconds.books, 1);
    assert_eq!(size.listening_seconds.total, 7200);
}

#[tokio::test]
async fn a_multi_volume_audiobook_sums_every_volume_of_the_resolved_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    // `sync` appends `MAX(ordinal) + 1` within a format, so a second multi-part
    // `.m4b` attaches to the book rather than colliding. Those are volumes of
    // one audiobook, all of them listenable — counting only the first reports a
    // third of the length while claiming the book as fully measured.
    let vol_one = seed_book(&pool, lib, "u-audio", "M4B", (None, None)).await;
    add_part(&pool, vol_one, 0, 36_000.0).await;
    for ordinal in [1, 2] {
        let vol = add_file(&pool, book_id_of(&pool, "u-audio").await, "M4B", ordinal).await;
        add_part(&pool, vol, 0, 36_000.0).await;
    }

    let size = sized(&pool).await;

    assert_eq!(size.listening_seconds.books, 1);
    assert_eq!(size.listening_seconds.total, 108_000);
}

#[tokio::test]
async fn a_multi_volume_audiobook_with_one_unprobed_volume_is_unmeasured() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let vol_one = seed_book(&pool, lib, "u-audio", "M4B", (None, None)).await;
    add_part(&pool, vol_one, 0, 36_000.0).await;
    // The all-or-nothing guard spans volumes: a book half-probed across two
    // files is no more measurable than one half-probed inside a single file.
    let vol_two = add_file(&pool, book_id_of(&pool, "u-audio").await, "M4B", 1).await;
    add_part(&pool, vol_two, 0, 0.0).await;

    let size = sized(&pool).await;

    assert!(size.listening_seconds.is_empty());
    assert_eq!(size.books, 1);
}

#[tokio::test]
async fn a_book_with_an_unprobed_part_is_unmeasured_rather_than_partly_counted() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let file = seed_book(&pool, lib, "u-half", "M4B", (None, None)).await;
    add_part(&pool, file, 0, 3600.0).await;
    // `duration_seconds` defaults to 0 until the indexer probes it. Counting
    // the probed half would report a 20-hour book as a 10-hour one *and* claim
    // it as covered.
    add_part(&pool, file, 1, 0.0).await;

    let size = sized(&pool).await;

    assert!(size.listening_seconds.is_empty());
    assert_eq!(size.books, 1);
}

#[tokio::test]
async fn a_book_held_in_two_audio_formats_counts_its_hours_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    // The M4B is ordinal 0, so it is the file the player resolves. Summing
    // both editions would report hours nobody can listen to.
    let m4b = seed_book(&pool, lib, "u-audio", "M4B", (None, None)).await;
    add_part(&pool, m4b, 0, 7200.0).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'u-audio'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mp3 = add_file(&pool, book_id, "MP3", 1).await;
    add_part(&pool, mp3, 0, 7000.0).await;

    let size = sized(&pool).await;

    assert_eq!(size.listening_seconds.books, 1);
    assert_eq!(size.listening_seconds.total, 7200);
}

// --- ghosted books ------------------------------------------------------

#[tokio::test]
async fn a_ghosted_book_is_absent_from_the_totals_and_from_the_denominator() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-live", "EPUB", (Some(90_000), None)).await;
    // Its file is gone, so its words aren't on disk. Counting them overstates
    // the library; counting it only in the denominator understates coverage
    // for a row nothing can ever measure.
    seed_ghost(&pool, lib, "u-ghost", Some(500_000)).await;

    let size = sized(&pool).await;

    assert_eq!(size.books, 1);
    assert_eq!(size.words.total, 90_000);
    assert_eq!(size.words.books, 1);
}

// --- the empty library --------------------------------------------------

#[tokio::test]
async fn a_library_with_nothing_measured_reports_empty_rather_than_zero() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB", (None, None)).await;

    let size = sized(&pool).await;

    // One book, nothing measured about it — an empty state, not "0 words".
    assert_eq!(size.books, 1);
    assert!(size.is_empty());
}

#[tokio::test]
async fn an_empty_library_reports_zero_books_and_no_measurements() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let size = sized(&pool).await;

    assert_eq!(size.books, 0);
    assert!(size.is_empty());
}

// --- caching ------------------------------------------------------------

/// A cache of this test's own. The process-wide entry has nothing to key it
/// on, so sharing it would race every other test in the binary — including the
/// worker suite, whose `Task::Scan` runs drop it on the way out.
fn test_cache() -> Cache {
    Cache::default()
}

#[tokio::test]
async fn library_size_in_serves_the_cached_answer_inside_the_ttl() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let cache = test_cache();
    seed_book(&pool, lib, "u-a", "EPUB", (Some(275), None)).await;

    assert_eq!(library_size_in(&cache, &pool, 0).await.unwrap().books, 1);

    seed_book(&pool, lib, "u-b", "EPUB", (Some(275), None)).await;
    // Inside the TTL the cached answer stands — that is the point of caching a
    // figure that only moves on a scan.
    assert_eq!(library_size_in(&cache, &pool, 1).await.unwrap().books, 1);
}

#[tokio::test]
async fn library_size_in_recomputes_once_the_entry_is_invalidated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let cache = test_cache();
    seed_book(&pool, lib, "u-a", "EPUB", (Some(275), None)).await;
    assert_eq!(library_size_in(&cache, &pool, 0).await.unwrap().books, 1);

    seed_book(&pool, lib, "u-b", "EPUB", (Some(275), None)).await;
    // A scan drops the entry, which is what stops a just-indexed library
    // reporting the size it had before.
    invalidate_in(&cache);

    assert_eq!(library_size_in(&cache, &pool, 1).await.unwrap().books, 2);
}

#[test]
fn store_if_current_drops_a_result_an_invalidate_overtook() {
    let cache = test_cache();
    let stale = LibrarySize {
        books: 1,
        ..LibrarySize::default()
    };

    // What a reader that entered `compute` before a scan finished would hold.
    let generation_at_start = cache.generation.load(Ordering::SeqCst);
    invalidate_in(&cache);

    store_if_current(&cache, generation_at_start, 0, stale);

    assert!(
        cache.entry.lock().unwrap().is_none(),
        "a pre-scan size must not be republished for the rest of the TTL"
    );
}

#[test]
fn store_if_current_publishes_a_result_no_invalidate_overtook() {
    let cache = test_cache();
    let size = LibrarySize {
        books: 2,
        ..LibrarySize::default()
    };

    let generation = cache.generation.load(Ordering::SeqCst);

    store_if_current(&cache, generation, 7, size);

    assert_eq!(*cache.entry.lock().unwrap(), Some((7, size)));
}

#[tokio::test]
async fn library_size_in_recomputes_once_the_ttl_has_passed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let cache = test_cache();
    seed_book(&pool, lib, "u-a", "EPUB", (Some(275), None)).await;
    assert_eq!(library_size_in(&cache, &pool, 0).await.unwrap().books, 1);

    seed_book(&pool, lib, "u-b", "EPUB", (Some(275), None)).await;

    // The TTL is the backstop for anything that resizes the library without
    // going through the worker.
    assert_eq!(
        library_size_in(&cache, &pool, LIBRARY_TTL_SECS)
            .await
            .unwrap()
            .books,
        2
    );
}

#[tokio::test]
async fn invalidate_clears_the_process_wide_entry() {
    // The `pub` entrypoint and its invalidation hook, over the real static —
    // the one place that has to touch it. No assertion on a *cached* value, so
    // nothing here races a concurrent scan in another test.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB", (Some(275), None)).await;

    assert_eq!(library_size(&pool).await.unwrap().books, 1);

    invalidate();

    assert!(cache().entry.lock().unwrap().is_none());
}

// --- error path ---------------------------------------------------------

#[tokio::test]
async fn compute_propagates_sqlx_error_when_the_books_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let err = compute(&pool).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}
