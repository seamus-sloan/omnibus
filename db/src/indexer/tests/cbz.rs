//! CBZ comics through the indexer: ComicInfo metadata and cover extraction,
//! uuid preservation when the file is removed, and a malformed archive
//! surfacing as an error row without aborting the scan.

use crate::pool::init_db;
use crate::test_support::{count_rows, make_test_dir, uuid_by_scan_key, CoversTempDir};

use super::super::*;

// ---------- #1562: CBZ ingestion through the normal diff/sync path ----------

/// Write a minimal real CBZ (ComicInfo.xml + one PNG page) at
/// `library_path/filename` so `reindex` exercises the actual comic parser.
fn write_cbz_at(library_path: &str, filename: &str) {
    let comic_info: &[u8] = br#"<ComicInfo>
  <Title>The Longing</Title>
  <Series>Berserk</Series>
  <Number>3</Number>
  <Writer>Kentaro Miura</Writer>
</ComicInfo>"#;
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 60, 50]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("png encode");
    let bytes =
        crate::test_support::build_stored_zip(&[("ComicInfo.xml", comic_info), ("p001.png", &png)]);
    std::fs::write(std::path::Path::new(library_path).join(filename), bytes).unwrap();
}

/// AC1: a CBZ in the ebook library indexes as a book with ComicInfo
/// title/author metadata, a first-page cover, and format `CBZ`.
#[tokio::test]
async fn reindex_indexes_a_cbz_with_comic_info_metadata_cover_and_cbz_format() {
    let _covers = CoversTempDir::new("reindex-cbz");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-cbz-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    write_cbz_at(&lib_path, "berserk-v03.cbz");

    reindex(&pool, &lib_path).await.unwrap();

    let (title, has_cover): (String, i64) =
        sqlx::query_as("SELECT title, has_cover FROM books WHERE scan_key = 'berserk-v03.cbz'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "The Longing");
    assert_eq!(has_cover, 1, "the first page becomes the cover");
    let format: String = sqlx::query_scalar(
        "SELECT bf.format FROM book_files bf
          JOIN books b ON b.id = bf.book_id WHERE b.scan_key = 'berserk-v03.cbz'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(format, "CBZ");
    let author: String = sqlx::query_scalar(
        "SELECT a.name FROM authors a
          JOIN books_authors_link l ON l.author = a.id
          JOIN books b ON b.id = l.book WHERE b.scan_key = 'berserk-v03.cbz'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(author, "Kentaro Miura");

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC2: reindex classifies an unchanged CBZ Unchanged (uuid preserved),
/// and removing the file ghosts the book — `book_files` dropped, the
/// `books` row (and its uuid) retained.
#[tokio::test]
async fn reindex_preserves_cbz_uuid_and_ghosts_the_book_when_the_file_is_removed() {
    let _covers = CoversTempDir::new("reindex-cbz-ghost");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-cbz-ghost-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    write_cbz_at(&lib_path, "berserk-v03.cbz");

    reindex(&pool, &lib_path).await.unwrap();
    let uuid = uuid_by_scan_key(&pool, "berserk-v03.cbz").await;

    reindex(&pool, &lib_path).await.unwrap();
    assert_eq!(
        uuid_by_scan_key(&pool, "berserk-v03.cbz").await,
        uuid,
        "a second scan preserves books.uuid"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        1,
        "the unchanged CBZ is not re-inserted"
    );

    std::fs::remove_file(lib.join("berserk-v03.cbz")).unwrap();
    reindex(&pool, &lib_path).await.unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0,
        "the removed file's book_files row is dropped"
    );
    assert_eq!(
        uuid_by_scan_key(&pool, "berserk-v03.cbz").await,
        uuid,
        "the ghosted books row keeps its identity"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC3: a malformed CBZ (not a zip) degrades to a metadata-less book row —
/// filename-fallback title, no cover — and the scan completes. (The parse
/// error itself lives on the in-memory `IndexedBook`, pinned by the
/// `comic::tests` failure-mode tests; the `books` schema has no error
/// column to persist it.)
#[tokio::test]
async fn reindex_surfaces_a_malformed_cbz_as_an_error_row_without_aborting() {
    let _covers = CoversTempDir::new("reindex-cbz-bad");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-cbz-bad-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    write_cbz_at(&lib_path, "good.cbz");
    std::fs::write(lib.join("bad.cbz"), b"not a zip").unwrap();

    reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        2,
        "the malformed archive does not abort the scan"
    );
    let good_title: String =
        sqlx::query_scalar("SELECT title FROM books WHERE scan_key = 'good.cbz'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(good_title, "The Longing", "the healthy CBZ still indexes");
    let (bad_title, bad_cover): (String, i64) =
        sqlx::query_as("SELECT title, has_cover FROM books WHERE scan_key = 'bad.cbz'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        bad_title, "bad.cbz",
        "the error row falls back to the filename title"
    );
    assert_eq!(bad_cover, 0, "the error row carries no cover");

    let _ = std::fs::remove_dir_all(&lib);
}
