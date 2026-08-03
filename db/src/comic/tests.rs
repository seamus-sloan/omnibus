//! Unit tests for the CBZ comic-archive parser: page listing + natural
//! sort, `ComicInfo.xml` mapping, filename fallback, the malformed-archive
//! failure modes (not a zip, zero pages), `extract_comic`'s persisted
//! `page_count` (#1593), and `page_count_for_book`'s three outcomes (real
//! archive, no CBZ file, unreadable archive).

use sqlx::SqlitePool;

use super::*;
use crate::test_support::{build_stored_zip, make_test_dir};

/// Tiny valid PNG (saturated color) so cover bytes decode and yield an
/// accent where it matters.
fn tiny_png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 60, 50]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("png encode");
    bytes
}

/// Write a CBZ built from `entries` into `dir` and return its path.
fn write_cbz(dir: &std::path::Path, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, build_stored_zip(entries)).expect("write cbz");
    path
}

const COMIC_INFO: &[u8] = br#"<?xml version="1.0"?>
<ComicInfo xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Title>The Longing</Title>
  <Series>Berserk</Series>
  <Number>3</Number>
  <Writer>Kentaro Miura</Writer>
  <PageCount>2</PageCount>
</ComicInfo>"#;

#[test]
fn extract_comic_maps_comic_info_fields_and_first_page_cover() {
    let dir = make_test_dir("comic-happy");
    let png = tiny_png();
    let path = write_cbz(
        &dir,
        "berserk-v03.cbz",
        &[
            ("ComicInfo.xml", COMIC_INFO),
            ("p002.png", b"second page"),
            ("p001.png", &png),
        ],
    );

    let book = extract_comic(&path, "berserk-v03.cbz".into(), &ScanOptions::default());

    assert_eq!(book.metadata.error, None);
    assert_eq!(book.metadata.title.as_deref(), Some("The Longing"));
    assert_eq!(book.metadata.series.as_deref(), Some("Berserk"));
    assert_eq!(book.metadata.series_index.as_deref(), Some("3"));
    assert_eq!(book.metadata.creators.len(), 1);
    assert_eq!(book.metadata.creators[0].name, "Kentaro Miura");
    // Persisted straight onto `books.page_count` (#1593), counted from the
    // archive's own page list — `ComicInfo.xml`'s `<PageCount>` tag (present
    // in this fixture) isn't a field the parser reads at all.
    assert_eq!(book.metadata.page_count, Some(2));
    // Cover = first page in natural order (p001), not zip entry order.
    let (mime, bytes) = book.cover.expect("first page becomes the cover");
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, png);
    assert!(
        book.metadata.accent.is_some(),
        "decodable cover yields an accent"
    );
    assert_eq!(book.word_count, None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_comic_falls_back_to_filename_stem_without_comic_info() {
    let dir = make_test_dir("comic-fallback");
    let path = write_cbz(
        &dir,
        "Kakuriyo v01 - Waco Ioka.cbz",
        &[("page1.jpg", b"jpeg-ish")],
    );

    let book = extract_comic(
        &path,
        "Kakuriyo v01 - Waco Ioka.cbz".into(),
        &ScanOptions::default(),
    );

    assert_eq!(book.metadata.error, None);
    assert_eq!(
        book.metadata.title.as_deref(),
        Some("Kakuriyo v01 - Waco Ioka")
    );
    assert!(book.metadata.creators.is_empty());
    assert_eq!(book.metadata.series, None);
    assert_eq!(book.metadata.page_count, Some(1));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_comic_returns_error_row_when_file_is_not_a_zip() {
    let dir = make_test_dir("comic-notzip");
    let path = dir.join("broken.cbz");
    std::fs::write(&path, b"not a zip").unwrap();

    let book = extract_comic(&path, "broken.cbz".into(), &ScanOptions::default());

    let err = book
        .metadata
        .error
        .expect("malformed archive surfaces error");
    assert!(err.contains("could not open cbz"), "got: {err}");
    assert_eq!(book.cover, None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_comic_returns_error_row_when_archive_has_no_image_pages() {
    let dir = make_test_dir("comic-nopages");
    let path = write_cbz(
        &dir,
        "empty.cbz",
        &[("ComicInfo.xml", COMIC_INFO), ("readme.txt", b"hi")],
    );

    let book = extract_comic(&path, "empty.cbz".into(), &ScanOptions::default());

    let err = book.metadata.error.expect("zero pages surfaces error");
    assert!(err.contains("no image pages"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_comic_prefers_a_sidecar_cover_over_the_first_page() {
    let dir = make_test_dir("comic-sidecar");
    let path = write_cbz(&dir, "book.cbz", &[("p001.png", b"embedded page")]);
    std::fs::write(dir.join("book.jpg"), b"sidecar bytes").unwrap();

    let book = extract_comic(&path, "book.cbz".into(), &ScanOptions::default());

    let (mime, bytes) = book.cover.expect("sidecar wins");
    assert_eq!(mime, "image/jpeg");
    assert_eq!(bytes, b"sidecar bytes");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_comic_materializes_the_first_page_as_a_sidecar_when_asked() {
    let dir = make_test_dir("comic-materialize");
    let png = tiny_png();
    let path = write_cbz(&dir, "book.cbz", &[("p001.png", &png)]);

    let opts = ScanOptions {
        materialize_sidecars: true,
    };
    let book = extract_comic(&path, "book.cbz".into(), &opts);

    assert!(book.cover.is_some());
    let sidecar = dir.join("book.png");
    assert_eq!(
        std::fs::read(&sidecar).expect("sidecar written"),
        png,
        "materialized sidecar carries the first page's bytes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_pages_returns_images_in_natural_sort_order() {
    let dir = make_test_dir("comic-pages");
    let path = write_cbz(
        &dir,
        "book.cbz",
        &[
            ("p10.jpg", b"a"),
            ("p2.jpg", b"b"),
            ("P1.JPG", b"c"),
            ("ComicInfo.xml", COMIC_INFO),
            ("notes.txt", b"skip me"),
        ],
    );

    let pages = list_pages(&path).unwrap();

    assert_eq!(pages, vec!["P1.JPG", "p2.jpg", "p10.jpg"]);
    assert_eq!(pages.len(), 3, "page count excludes non-image entries");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_pages_excludes_plain_dot_prefixed_names_same_as_appledouble_forks() {
    let dir = make_test_dir("comic-pages-hidden-broad");
    let path = write_cbz(
        &dir,
        "book.cbz",
        &[
            // The canonical case this filter was written for: a macOS
            // AppleDouble resource fork sitting next to a real page.
            ("__MACOSX/._page1.jpg", b"appledouble fork"),
            // Not AppleDouble at all — a directory an archiver could
            // plausibly name for real content — but excluded too, since
            // `is_hidden_name` treats any leading dot as hidden.
            (".chapter1/page1.jpg", b"legit dot-prefixed name"),
            ("page1.jpg", b"real page"),
        ],
    );

    let pages = list_pages(&path).unwrap();

    assert_eq!(
        pages,
        vec!["page1.jpg"],
        "both the AppleDouble fork and the plain dot-prefixed name are excluded"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_pages_excludes_hidden_entries_and_appledouble_forks() {
    let dir = make_test_dir("comic-pages-hidden");
    let path = write_cbz(
        &dir,
        "book.cbz",
        &[
            ("__MACOSX/._p001.png", b"appledouble fork"),
            (".hidden.png", b"hidden"),
            ("art/.thumbs/t1.jpg", b"hidden dir"),
            ("p001.png", b"real cover"),
        ],
    );

    let pages = list_pages(&path).unwrap();

    assert_eq!(
        pages,
        vec!["p001.png"],
        "hidden segments never count as pages"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_page_returns_mime_and_bytes_in_natural_order() {
    let dir = make_test_dir("comic-read-page");
    let path = write_cbz(
        &dir,
        "book.cbz",
        &[
            ("p10.jpg", b"tenth"),
            ("p2.png", b"second"),
            ("p1.jpg", b"first"),
        ],
    );

    let (mime, bytes) = read_page(&path, 1).unwrap().expect("page 1 exists");

    assert_eq!(mime, "image/png");
    assert_eq!(bytes, b"second");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_page_returns_none_when_index_is_out_of_range() {
    let dir = make_test_dir("comic-read-page-oob");
    let path = write_cbz(&dir, "book.cbz", &[("p1.jpg", b"only page")]);

    assert!(read_page(&path, 1).unwrap().is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_page_errors_when_a_page_exceeds_the_decompressed_byte_cap() {
    let dir = make_test_dir("comic-read-page-cap");
    // One byte past the (test-sized) cap — the bounded read must refuse it
    // rather than buffer whatever the archive claims.
    let oversized = vec![0u8; MAX_PAGE_BYTES as usize + 1];
    let path = write_cbz(&dir, "bomb.cbz", &[("p1.jpg", &oversized)]);

    let err = read_page(&path, 0).expect_err("over-cap page must error");
    assert!(err.to_string().contains("byte cap"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_page_propagates_error_when_file_is_not_a_zip() {
    let dir = make_test_dir("comic-read-page-err");
    let path = dir.join("broken.cbz");
    std::fs::write(&path, b"not a zip").unwrap();

    assert!(read_page(&path, 0).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_pages_propagates_error_when_file_is_not_a_zip() {
    let dir = make_test_dir("comic-pages-err");
    let path = dir.join("broken.cbz");
    std::fs::write(&path, b"not a zip").unwrap();

    assert!(list_pages(&path).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_comic_info_reads_fields_and_splits_writers_on_commas() {
    let info = parse_comic_info(
        br#"<ComicInfo><Title>T</Title><Series>S</Series><Number>1.5</Number>
            <Writer>Alice Ink, Bob Quill</Writer></ComicInfo>"#,
    );
    assert_eq!(info.title.as_deref(), Some("T"));
    assert_eq!(info.series.as_deref(), Some("S"));
    assert_eq!(info.number.as_deref(), Some("1.5"));
    let writers = contributors_from_writer(info.writer.as_deref().unwrap());
    assert_eq!(
        writers.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["Alice Ink", "Bob Quill"]
    );
}

#[test]
fn parse_comic_info_tolerates_malformed_xml() {
    let info = parse_comic_info(b"<ComicInfo><Title>Kept</Title><Broken");
    assert_eq!(info.title.as_deref(), Some("Kept"));
    assert_eq!(info.series, None);
}

#[test]
fn natural_cmp_orders_digit_runs_numerically() {
    use std::cmp::Ordering;
    assert_eq!(natural_cmp("p2.jpg", "p10.jpg"), Ordering::Less);
    assert_eq!(natural_cmp("p007.jpg", "p8.jpg"), Ordering::Less);
    assert_eq!(
        natural_cmp("A1.png", "a1.PNG"),
        natural_cmp("A1.png", "a1.PNG")
    );
    assert_eq!(natural_cmp("ch1/p1.jpg", "ch1/p1.jpg"), Ordering::Equal);
    // Leading-zero tie: equal numerically, shorter run sorts first.
    assert_eq!(natural_cmp("p7.jpg", "p07.jpg"), Ordering::Less);
}

#[test]
fn is_comic_path_matches_cbz_case_insensitively() {
    assert!(is_comic_path(std::path::Path::new("a/b.cbz")));
    assert!(is_comic_path(std::path::Path::new("a/b.CBZ")));
    assert!(!is_comic_path(std::path::Path::new("a/b.epub")));
    assert!(!is_comic_path(std::path::Path::new("cbz")));
}

/// Seed one book whose `book_files` CBZ row resolves (via `book_file_path`)
/// to `<lib>/sub/book.cbz`, writing `bytes` there. Mirrors
/// `test_support::seed_epub_book_at`'s shape for the CBZ format
/// `page_count_for_book` reads. Returns the new `books.id`.
async fn seed_cbz_book_at(pool: &SqlitePool, lib: &std::path::Path, bytes: &[u8]) -> i64 {
    let lib_str = lib.to_string_lossy().to_string();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(&lib_str)
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(&lib_str)
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('uuid-cbz', 'sub/book.cbz', ?, 'sub', 'Title') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'CBZ', 'book', ?, 1)",
    )
    .bind(book_id)
    .bind(bytes.len() as i64)
    .execute(pool)
    .await
    .unwrap();
    std::fs::create_dir_all(lib.join("sub")).unwrap();
    std::fs::write(lib.join("sub").join("book.cbz"), bytes).unwrap();
    book_id
}

#[tokio::test]
async fn page_count_for_book_counts_pages_in_a_real_cbz() {
    let dir = make_test_dir("comic-page-count-happy");
    let bytes = build_stored_zip(&[("p1.jpg", b"one"), ("p2.jpg", b"two"), ("p3.jpg", b"three")]);
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_cbz_book_at(&pool, &dir, &bytes).await;

    let count = page_count_for_book(&pool, book_id).await;

    assert_eq!(count, Some(3));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn page_count_for_book_returns_none_when_book_has_no_cbz_file() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // Book row exists but carries no `book_files` row of any format, let
    // alone CBZ — `book_file_path` resolves to `None` and the count must
    // read as "no count", not an error.
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('uuid-nofile', 'sub/book', ?, 'sub', 'Title') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let count = page_count_for_book(&pool, book_id).await;

    assert_eq!(count, None);
}

#[tokio::test]
async fn page_count_for_book_returns_none_when_archive_is_unreadable() {
    let dir = make_test_dir("comic-page-count-unreadable");
    // `book_files` points at a file that isn't a valid zip — the archive
    // open fails inside the blocking task, and that must degrade to `None`
    // rather than propagate an error into the detail read.
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_cbz_book_at(&pool, &dir, b"not a zip").await;

    let count = page_count_for_book(&pool, book_id).await;

    assert_eq!(count, None);

    let _ = std::fs::remove_dir_all(&dir);
}
