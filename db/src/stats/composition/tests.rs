//! Unit tests for the library-composition rollups. Every one is really a test
//! about **counting books rather than rows**: `book_files` is one row per
//! file, a genre override is one JSON element per genre, and the naive
//! `COUNT(*)` turns a twelve-part audiobook into a shelf.

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

/// One `books` row with an optional `pubdate`, and no files yet.
async fn seed_row(pool: &SqlitePool, lib_id: i64, uuid: &str, pubdate: Option<&str>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, pubdate)
         VALUES (?, ?, '', ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .bind(pubdate)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One live book: a `books` row plus one `book_files` row.
async fn seed_book(pool: &SqlitePool, lib_id: i64, uuid: &str, format: &str) -> i64 {
    let book_id = seed_row(pool, lib_id, uuid, None).await;
    add_file(pool, book_id, format, 0).await;
    book_id
}

async fn add_file(pool: &SqlitePool, book_id: i64, format: &str, ordinal: i64) {
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes)
         VALUES (?, ?, 'f', ?, 0)",
    )
    .bind(book_id)
    .bind(format)
    .bind(ordinal)
    .execute(pool)
    .await
    .unwrap();
}

async fn link_language(pool: &SqlitePool, book_id: i64, code: &str) {
    link(
        pool,
        book_id,
        "languages",
        "code",
        code,
        "books_languages_link",
        "language",
    )
    .await;
}

async fn link_publisher(pool: &SqlitePool, book_id: i64, name: &str) {
    link(
        pool,
        book_id,
        "publishers",
        "name",
        name,
        "books_publishers_link",
        "publisher",
    )
    .await;
}

async fn link(
    pool: &SqlitePool,
    book_id: i64,
    entity_table: &str,
    name_col: &str,
    value: &str,
    link_table: &str,
    fk: &str,
) {
    sqlx::query(&format!(
        "INSERT OR IGNORE INTO {entity_table} ({name_col}) VALUES (?)"
    ))
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO {link_table} (book, {fk})
         SELECT ?, id FROM {entity_table} WHERE {name_col} = ?"
    ))
    .bind(book_id)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

/// Assign genres the only way a book can have them: a `metadata_overrides`
/// JSON array plus the vocabulary rows the rollup joins for canonical case.
async fn set_genres(pool: &SqlitePool, uuid: &str, names: &[&str]) {
    for name in names {
        sqlx::query("INSERT OR IGNORE INTO genres (name) VALUES (?)")
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
    }
    let json = serde_json::to_string(names).unwrap();
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides)
         VALUES (?, json_object('genres', json(?)))
         ON CONFLICT(book_uuid) DO UPDATE SET overrides = excluded.overrides",
    )
    .bind(uuid)
    .bind(json)
    .execute(pool)
    .await
    .unwrap();
}

/// Slice counts by label, for assertions that don't care about order.
fn by_label(dim: &CompositionDimension) -> std::collections::HashMap<String, i64> {
    dim.slices
        .iter()
        .map(|s| (s.label.clone(), s.books))
        .collect()
}

/// `compute` directly: the cache is a process-wide `static`, so the tests that
/// aren't *about* caching must not go through it.
async fn composed(pool: &SqlitePool) -> LibraryComposition {
    compute(pool).await.unwrap()
}

// --- formats: one row per file, not per book ----------------------------

#[tokio::test]
async fn a_multi_part_audiobook_counts_as_one_book_in_the_format_mix() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let book = seed_book(&pool, lib, "u-audio", "M4B").await;
    // Migration `0018` keys `book_files` UNIQUE(book_id, format, ordinal), so
    // a twelve-part rip is twelve rows. `COUNT(*)` would call it a shelf.
    for ordinal in 1..12 {
        add_file(&pool, book, "M4B", ordinal).await;
    }

    let c = composed(&pool).await;

    assert_eq!(by_label(&c.formats).get("M4B"), Some(&1));
    assert_eq!(c.formats.coverage.books, 1);
    assert_eq!(c.formats.coverage.total, 1);
    assert_eq!(c.books, 1);
}

#[tokio::test]
async fn a_dual_format_book_counts_once_in_each_bucket_and_reports_the_overlap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let both = seed_book(&pool, lib, "u-both", "EPUB").await;
    add_file(&pool, both, "M4B", 0).await;
    seed_book(&pool, lib, "u-epub", "EPUB").await;

    let c = composed(&pool).await;

    // It really is both, so it appears in both buckets — and the overlap is
    // published rather than left for a reader to discover by adding up.
    assert_eq!(by_label(&c.formats).get("EPUB"), Some(&2));
    assert_eq!(by_label(&c.formats).get("M4B"), Some(&1));
    assert_eq!(c.formats.coverage.books, 2);
    assert_eq!(
        c.formats.coverage.total, 3,
        "three placements over two books"
    );
    assert_eq!(c.formats.overlap(), 1);
    // The identity the surfaces publish: placements sum to `coverage.total`.
    let summed: i64 = c.formats.slices.iter().map(|s| s.books).sum();
    assert_eq!(summed, c.formats.coverage.total);
}

#[tokio::test]
async fn format_buckets_fold_case_variants_into_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB").await;
    seed_book(&pool, lib, "u-b", "epub").await;

    let c = composed(&pool).await;

    assert_eq!(c.formats.slices.len(), 1);
    assert_eq!(by_label(&c.formats).get("EPUB"), Some(&2));
}

// --- ghosted books ------------------------------------------------------

#[tokio::test]
async fn a_ghosted_book_is_reported_rather_than_silently_dropped_from_the_format_mix() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-live", "EPUB").await;
    // A `books` row whose files are gone. It carries no format at all, so it
    // would vanish from the rollup and leave the counts failing to reconcile.
    seed_row(&pool, lib, "u-ghost", None).await;

    let c = composed(&pool).await;

    assert_eq!(c.books, 1, "live books only");
    assert_eq!(c.ghosted_books, 1);
    assert_eq!(by_label(&c.formats).get("EPUB"), Some(&1));
    // The reconciliation: live + ghosted is every `books` row.
    let all: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(c.books + c.ghosted_books, all);
}

#[tokio::test]
async fn a_ghosted_book_reaches_no_dimension_at_all() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let ghost = seed_row(&pool, lib, "u-ghost", Some("1999-04-02")).await;
    link_language(&pool, ghost, "eng").await;
    link_publisher(&pool, ghost, "Gone Press").await;
    set_genres(&pool, "u-ghost", &["Horror"]).await;

    let c = composed(&pool).await;

    assert_eq!(c.books, 0);
    assert_eq!(c.ghosted_books, 1);
    for dim in [&c.languages, &c.publishers, &c.decades, &c.genres] {
        assert!(dim.is_empty(), "{dim:?}");
    }
}

// --- decades ------------------------------------------------------------

#[tokio::test]
async fn decades_bucket_on_the_same_year_extraction_smart_shelves_use() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    for (uuid, pubdate) in [
        ("u-a", "1994-07-01"),
        ("u-b", "1999"),
        ("u-c", "2021-11-30T00:00:00+00:00"),
    ] {
        let id = seed_row(&pool, lib, uuid, Some(pubdate)).await;
        add_file(&pool, id, "EPUB", 0).await;
    }

    let c = composed(&pool).await;

    // Oldest first — a histogram sorted by height is a bar chart of nothing.
    assert_eq!(
        c.decades
            .slices
            .iter()
            .map(|s| (s.label.as_str(), s.books))
            .collect::<Vec<_>>(),
        vec![("1990s", 2), ("2020s", 1)]
    );
    assert_eq!(c.decades.coverage.books, 3);
    assert_eq!(c.decades.uncovered(c.books), 0);
}

#[tokio::test]
async fn an_unparseable_or_absent_pubdate_is_unknown_rather_than_bucketed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    for (uuid, pubdate) in [
        ("u-dated", Some("1984-01-01")),
        // `CAST('n.d.' AS INTEGER)` is 0 in SQLite, and so is an empty string
        // — bucketing either would invent a decade around the year zero.
        ("u-junk", Some("n.d.")),
        ("u-blank", Some("")),
        ("u-none", None),
        // Calibre's UNDEFINED_DATE sentinel. Parses cleanly to 101, which is
        // exactly why a bare "is it a number?" check files it under the 100s.
        ("u-sentinel", Some("0101-01-01T00:00:00+00:00")),
    ] {
        let id = seed_row(&pool, lib, uuid, pubdate).await;
        add_file(&pool, id, "EPUB", 0).await;
    }

    let c = composed(&pool).await;

    assert_eq!(
        c.decades
            .slices
            .iter()
            .map(|s| s.label.as_str())
            .collect::<Vec<_>>(),
        vec!["1980s"],
        "only the one real date may become a bucket"
    );
    assert_eq!(c.books, 5);
    assert_eq!(c.decades.coverage.books, 1);
    assert_eq!(c.decades.uncovered(c.books), 4, "the four unknowns");
}

#[tokio::test]
async fn a_library_with_no_publication_dates_reports_the_decades_dimension_empty() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB").await;

    let c = composed(&pool).await;

    // An empty state, not a single 100%-wide "Unknown" bar.
    assert!(c.decades.is_empty());
    assert_eq!(c.decades.uncovered(c.books), 1);
}

// --- languages & publishers --------------------------------------------

#[tokio::test]
async fn languages_and_publishers_count_distinct_live_books_and_report_their_coverage() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let a = seed_book(&pool, lib, "u-a", "EPUB").await;
    let b = seed_book(&pool, lib, "u-b", "EPUB").await;
    seed_book(&pool, lib, "u-c", "EPUB").await;
    link_language(&pool, a, "eng").await;
    link_language(&pool, b, "eng").await;
    // A bilingual edition is one book in two buckets, so the placement count
    // runs ahead of the book count by exactly one.
    link_language(&pool, b, "fra").await;
    link_publisher(&pool, a, "Tor").await;

    let c = composed(&pool).await;

    assert_eq!(by_label(&c.languages).get("eng"), Some(&2));
    assert_eq!(by_label(&c.languages).get("fra"), Some(&1));
    assert_eq!(c.languages.coverage.books, 2);
    assert_eq!(c.languages.coverage.total, 3);
    assert_eq!(c.languages.overlap(), 1);
    // The third book declared no language: uncovered, never an invented
    // "Unknown" bucket.
    assert_eq!(c.languages.uncovered(c.books), 1);

    assert_eq!(by_label(&c.publishers).get("Tor"), Some(&1));
    assert_eq!(c.publishers.uncovered(c.books), 2);
}

#[tokio::test]
async fn a_dimension_nothing_carries_reports_empty_rather_than_an_empty_chart() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB").await;

    let c = composed(&pool).await;

    assert!(c.publishers.is_empty());
    assert!(c.languages.is_empty());
    assert!(c.genres.is_empty());
    // The format mix still has something to say, so the card is not empty.
    assert!(!c.formats.is_empty());
    assert!(!c.is_empty());
}

#[tokio::test]
async fn an_open_ended_dimension_folds_its_tail_into_one_other_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    for i in 0..(SLICE_LIMIT + 3) {
        let uuid = format!("u-{i}");
        let id = seed_book(&pool, lib, &uuid, "EPUB").await;
        link_publisher(&pool, id, &format!("Press {i}")).await;
    }

    let c = composed(&pool).await;

    assert_eq!(c.publishers.slices.len(), SLICE_LIMIT + 1);
    assert_eq!(c.publishers.slices.last().unwrap().label, OTHER_LABEL);
    assert_eq!(c.publishers.slices.last().unwrap().books, 3);
    // Folding must not disturb the sum the coverage pair promises.
    let summed: i64 = c.publishers.slices.iter().map(|s| s.books).sum();
    assert_eq!(summed, c.publishers.coverage.total);
}

// --- genres -------------------------------------------------------------

#[tokio::test]
async fn genres_publish_the_override_coverage_behind_the_distribution() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    for i in 0..10 {
        seed_book(&pool, lib, &format!("u-{i}"), "EPUB").await;
    }
    // Two books hand-edited out of ten. Presenting these slices as "your
    // library's genres" is the failure the coverage pair exists to prevent.
    set_genres(&pool, "u-0", &["Fantasy", "Horror"]).await;
    set_genres(&pool, "u-1", &["Fantasy"]).await;

    let c = composed(&pool).await;

    assert_eq!(c.books, 10);
    assert_eq!(by_label(&c.genres).get("Fantasy"), Some(&2));
    assert_eq!(by_label(&c.genres).get("Horror"), Some(&1));
    assert_eq!(c.genres.coverage.books, 2, "books with any genre at all");
    assert_eq!(c.genres.coverage.total, 3, "genre placements");
    assert_eq!(c.genres.uncovered(c.books), 8);
}

#[tokio::test]
async fn genre_case_variants_fold_into_one_slice() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "EPUB").await;
    seed_book(&pool, lib, "u-b", "EPUB").await;
    set_genres(&pool, "u-a", &["Sci-Fi"]).await;
    // The vocabulary table is UNIQUE COLLATE NOCASE, so "sci-fi" resolves to
    // the same row — a donut that split the two would double-count a slice.
    set_genres(&pool, "u-b", &["sci-fi"]).await;

    let c = composed(&pool).await;

    assert_eq!(c.genres.slices.len(), 1);
    assert_eq!(c.genres.slices[0].books, 2);
    assert_eq!(c.genres.coverage.books, 2);
}

// --- the empty library --------------------------------------------------

#[tokio::test]
async fn an_empty_library_reports_nothing_to_describe() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let c = composed(&pool).await;

    assert_eq!(c.books, 0);
    assert_eq!(c.ghosted_books, 0);
    assert!(c.is_empty());
}

// --- caching ------------------------------------------------------------

/// A cache of this test's own. The process-wide entry has nothing to key it
/// on, so sharing it would race every other test in the binary — including the
/// worker suite, whose `Task::Scan` runs drop it on the way out.
fn test_cache() -> Cache {
    Cache::default()
}

#[tokio::test]
async fn library_composition_in_serves_a_fresh_entry_and_recomputes_once_invalidated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let cache = test_cache();
    seed_book(&pool, lib, "u-a", "EPUB").await;

    assert_eq!(
        library_composition_in(&cache, &pool, 0)
            .await
            .unwrap()
            .books,
        1
    );

    seed_book(&pool, lib, "u-b", "EPUB").await;
    // Inside the TTL the cached answer stands — the point of caching a figure
    // that only moves on a scan.
    assert_eq!(
        library_composition_in(&cache, &pool, 1)
            .await
            .unwrap()
            .books,
        1
    );
    // A scan drops the entry, which is what stops a just-indexed library
    // describing the collection it held before.
    invalidate_in(&cache);
    assert_eq!(
        library_composition_in(&cache, &pool, 1)
            .await
            .unwrap()
            .books,
        2
    );
}

#[test]
fn store_if_current_drops_a_result_an_invalidate_overtook() {
    let cache = test_cache();
    let stale = LibraryComposition {
        books: 1,
        ..LibraryComposition::default()
    };

    // What a reader that entered `compute` before a scan finished would hold.
    let generation_at_start = cache.generation.load(Ordering::SeqCst);
    invalidate_in(&cache);

    store_if_current(&cache, generation_at_start, 0, &stale);

    assert!(
        cache.entry.lock().unwrap().is_none(),
        "a pre-scan composition must not be republished for the rest of the TTL"
    );
}

#[test]
fn store_if_current_publishes_a_result_no_invalidate_overtook() {
    let cache = test_cache();
    let composition = LibraryComposition {
        books: 2,
        ..LibraryComposition::default()
    };

    let generation = cache.generation.load(Ordering::SeqCst);

    store_if_current(&cache, generation, 7, &composition);

    assert_eq!(
        cache.entry.lock().unwrap().as_ref(),
        Some(&(7, composition))
    );
}

#[tokio::test]
async fn library_composition_in_recomputes_once_the_ttl_has_passed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = seed_lib(&pool).await;
    let cache = test_cache();
    seed_book(&pool, lib, "u-a", "EPUB").await;
    assert_eq!(
        library_composition_in(&cache, &pool, 0)
            .await
            .unwrap()
            .books,
        1
    );

    seed_book(&pool, lib, "u-b", "EPUB").await;

    // The TTL is the backstop for anything that changes the library without
    // going through the worker.
    assert_eq!(
        library_composition_in(&cache, &pool, COMPOSITION_TTL_SECS)
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
    seed_book(&pool, lib, "u-a", "EPUB").await;

    assert_eq!(library_composition(&pool).await.unwrap().books, 1);

    invalidate();

    assert!(cache().entry.lock().unwrap().is_none());
}

// --- folding & error path ----------------------------------------------

#[test]
fn fold_tail_keeps_a_short_list_untouched() {
    let slices: Vec<CompositionSlice> = (0..SLICE_LIMIT)
        .map(|i| CompositionSlice {
            label: format!("s{i}"),
            books: 1,
        })
        .collect();

    assert_eq!(fold_tail(slices.clone()), slices);
    assert_eq!(fold_tail(Vec::new()), Vec::new());
}

#[test]
fn fold_tail_absorbs_the_tail_into_a_real_slice_already_named_other() {
    // A publisher genuinely named "Other" outranking the fold boundary. Two
    // bars of that name is not just an odd legend: `CompositionSlice` is
    // `Identifiable` by label on iOS, and a duplicate id makes SwiftUI's
    // `ForEach` render undefined results.
    let mut slices = vec![CompositionSlice {
        label: OTHER_LABEL.to_string(),
        books: 50,
    }];
    slices.extend((0..SLICE_LIMIT).map(|i| CompositionSlice {
        label: format!("p{i}"),
        books: 2,
    }));
    let total: i64 = slices.iter().map(|s| s.books).sum();

    let folded = fold_tail(slices);

    assert_eq!(
        folded.iter().filter(|s| s.label == OTHER_LABEL).count(),
        1,
        "the synthetic tail must merge into the real row, not sit beside it"
    );
    // Folding never moves the sum the coverage pair is read against.
    assert_eq!(folded.iter().map(|s| s.books).sum::<i64>(), total);
    assert_eq!(folded[0].books, 52);
}

#[tokio::test]
async fn library_composition_propagates_sqlx_error_when_the_books_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let err = compute(&pool).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}
