//! Wire content validators (see `.claude/rules/09-content-validators.md`):
//! the `etag` `get_book`/`get_book_files` derive from the scanner's stat, and
//! the batched `download_validators` lookup.

use crate::pool::init_db;

use super::super::*;

#[tokio::test]
async fn get_book_files_publishes_a_content_validator_derived_from_the_stat() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 4096, 255)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let files = get_book_files(&pool, book_id).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].etag.as_deref(),
        Some("\"ff-1000\""),
        "the wire validator is the (mtime_epoch, size_bytes) pair the reindex diff keys on"
    );
}

#[tokio::test]
async fn get_book_files_omits_the_validator_for_a_row_the_scanner_has_not_stat_ed() {
    // `(0, 0)` is the indexer's never-observed sentinel. Publishing a
    // validator for it would make the one-time stat backfill look like a
    // content change on every device holding a download.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 0, 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let files = get_book_files(&pool, book_id).await.unwrap();
    assert_eq!(files[0].etag, None);
}

#[tokio::test]
async fn get_book_publishes_the_validator_for_an_ordinary_single_file_book() {
    // The case that matters most and was previously omitted: `book_files`
    // used to be withheld unless some format had more than one row, so a
    // typical one-EPUB book reached clients with no validator at all — and
    // a whole library of those is the normal shape. Every offline staleness
    // check reads this field, so withholding it here disables the feature
    // for exactly the common case.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 4096, 255)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, book_id).await.unwrap().expect("book");
    assert_eq!(
        book.book_files.len(),
        1,
        "a single-file book still lists its file"
    );
    assert_eq!(book.book_files[0].etag.as_deref(), Some("\"ff-1000\""));
}

#[tokio::test]
async fn get_book_publishes_a_validator_per_file_on_a_dual_format_book() {
    // One EPUB + one M4B is still one row per format, so this was withheld
    // too — leaving a dual-format book unable to detect staleness on either
    // of its downloads.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for (format, size, mtime) in [("EPUB", 4096, 255), ("M4B", 8192, 511)] {
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
             VALUES (?, ?, 'b', ?, ?)",
        )
        .bind(book_id)
        .bind(format)
        .bind(size)
        .bind(mtime)
        .execute(&pool)
        .await
        .unwrap();
    }

    let book = get_book(&pool, book_id).await.unwrap().expect("book");
    assert_eq!(book.book_files.len(), 2);
    assert!(
        book.book_files.iter().all(|f| f.etag.is_some()),
        "each format's download needs its own validator"
    );
}

#[tokio::test]
async fn get_book_serializes_the_validator_onto_the_wire() {
    // The projection carrying an etag is not the same as clients receiving
    // one — `book_files` is `skip_serializing_if = "Vec::is_empty"`, and the
    // etag itself is skipped when absent. Pin the actual JSON.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 4096, 255)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, book_id).await.unwrap().expect("book");
    let wire: serde_json::Value = serde_json::to_value(&book).unwrap();
    assert_eq!(
        wire["book_files"][0]["etag"].as_str(),
        Some("\"ff-1000\""),
        "clients read the validator off this exact path"
    );
}

/// Seed one book with the given `(format, ordinal, size, mtime)` files.
async fn seed_book_with_files(
    pool: &sqlx::SqlitePool,
    uuid: &str,
    files: &[(&str, i64, i64, i64)],
) -> i64 {
    sqlx::query(
        "INSERT OR IGNORE INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')",
    )
    .execute(pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES (?, ?, 1, '/lib/b', 'Book') RETURNING id",
    )
    .bind(uuid)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    for (format, ordinal, size, mtime) in files {
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes, mtime_epoch) \
             VALUES (?, ?, 'b', ?, ?, ?)",
        )
        .bind(book_id)
        .bind(*format)
        .bind(*ordinal)
        .bind(*size)
        .bind(*mtime)
        .execute(pool)
        .await
        .unwrap();
    }
    book_id
}

fn validator_query(
    uuid: &str,
    format: omnibus_shared::DownloadFormat,
    file_id: Option<i64>,
) -> omnibus_shared::DownloadValidatorQuery {
    omnibus_shared::DownloadValidatorQuery {
        book_uuid: uuid.into(),
        format,
        file_id,
    }
}

#[tokio::test]
async fn download_validators_answers_each_query_about_the_file_the_server_would_serve() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Two EPUB editions and two audio parts, so "which row" is a real
    // question rather than the only row present.
    seed_book_with_files(
        &pool,
        "bk",
        &[
            ("EPUB", 1, 4096, 255),
            ("EPUB", 0, 8192, 511),
            ("M4B", 1, 1024, 15),
            ("M4B", 0, 2048, 31),
        ],
    )
    .await;

    let answers = download_validators(
        &pool,
        &[
            validator_query("bk", DownloadFormat::Epub, None),
            validator_query("bk", DownloadFormat::Audio, None),
        ],
    )
    .await
    .unwrap();

    // Lowest ordinal of the format, matching `book_file_path`'s
    // `ORDER BY bf.ordinal LIMIT 1` — answering about the other edition
    // would report a download stale that isn't.
    assert_eq!(answers[0].etag.as_deref(), Some("\"1ff-2000\""));
    assert_eq!(answers[1].etag.as_deref(), Some("\"1f-800\""));
}

#[tokio::test]
async fn download_validators_honours_an_explicitly_chosen_file() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book_with_files(
        &pool,
        "bk",
        &[("EPUB", 0, 8192, 511), ("EPUB", 1, 4096, 255)],
    )
    .await;
    let second: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? AND ordinal = 1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let answers = download_validators(
        &pool,
        &[validator_query("bk", DownloadFormat::Epub, Some(second))],
    )
    .await
    .unwrap();

    assert_eq!(
        answers[0].etag.as_deref(),
        Some("\"ff-1000\""),
        "an explicit file_id must win over the default row"
    );
}

#[tokio::test]
async fn download_validators_reports_no_etag_for_anything_it_cannot_answer() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_book_with_files(&pool, "bk", &[("EPUB", 0, 4096, 255), ("M4B", 0, 0, 0)]).await;

    let answers = download_validators(
        &pool,
        &[
            // Unknown book.
            validator_query("gone", DownloadFormat::Epub, None),
            // Known book, but no file of that format.
            validator_query("bk", DownloadFormat::Audio, Some(9999)),
            // Known file the scanner has never stat'd — the (0, 0) sentinel.
            validator_query("bk", DownloadFormat::Audio, None),
        ],
    )
    .await
    .unwrap();

    assert!(answers.iter().all(|a| a.etag.is_none()));
    // The shape still round-trips, so a client can line answers up with the
    // questions it asked.
    assert_eq!(answers[0].book_uuid, "gone");
    assert_eq!(answers[1].file_id, Some(9999));
}

#[tokio::test]
async fn download_validators_answers_a_batch_in_order() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    for (uuid, mtime) in [("bk-1", 255), ("bk-2", 511), ("bk-3", 767)] {
        seed_book_with_files(&pool, uuid, &[("EPUB", 0, 4096, mtime)]).await;
    }

    let queries: Vec<_> = ["bk-3", "bk-1", "bk-2"]
        .iter()
        .map(|u| validator_query(u, DownloadFormat::Epub, None))
        .collect();
    let answers = download_validators(&pool, &queries).await.unwrap();

    let uuids: Vec<&str> = answers.iter().map(|a| a.book_uuid.as_str()).collect();
    assert_eq!(
        uuids,
        ["bk-3", "bk-1", "bk-2"],
        "answers ride with their questions"
    );
    assert_eq!(answers[0].etag.as_deref(), Some("\"2ff-1000\""));
}

#[tokio::test]
async fn download_validators_resolves_a_merged_uuid_to_the_surviving_book() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book_with_files(&pool, "bk", &[("EPUB", 0, 4096, 255)]).await;
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key) \
         VALUES ('old-uuid', ?, 'EPUB', '/lib', 'b.epub')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let answers = download_validators(
        &pool,
        &[validator_query("old-uuid", DownloadFormat::Epub, None)],
    )
    .await
    .unwrap();

    assert_eq!(
        answers[0].etag.as_deref(),
        Some("\"ff-1000\""),
        "a download taken before a merge must still be answerable"
    );
}
