//! Tests for the export-with-overrides EPUB rewrite, split by sub-topic
//! into the sibling modules below; the sample OPF, override and PNG
//! fixtures they share live here, alongside `seed_epub_row`, which the
//! worker tests reuse.

mod archive;
mod bulk;
mod cache;
mod opf_cover;

use std::io::Cursor;

use epub::doc::EpubDoc;
use image::{DynamicImage, ImageFormat, RgbImage};

/// A solid-color PNG of the given size — a stand-in override cover whose
/// dimensions are recognizable in a rewritten archive.
fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut img = RgbImage::new(w, h);
    for px in img.pixels_mut() {
        *px = image::Rgb(rgb);
    }
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// A representative OPF: an identifier anchoring the package
/// `unique-identifier`, managed `<dc:*>` fields, a `calibre:series` meta, the
/// EPUB2 `<meta name="cover">` pointer, an EPUB3 `dcterms:modified` refinement,
/// and a manifest + spine — everything the rewrite must either replace or
/// preserve.
const SAMPLE_OPF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="bookid" opf:scheme="uuid">urn:uuid:ORIG-UUID-9f3</dc:identifier>
    <dc:title>Original Title</dc:title>
    <dc:creator opf:role="aut">Original Author</dc:creator>
    <dc:language>en</dc:language>
    <dc:subject>OldSubject</dc:subject>
    <meta property="dcterms:modified">2020-01-01T00:00:00Z</meta>
    <meta name="calibre:series" content="Old Series"/>
    <meta name="cover" content="cover-img"/>
  </metadata>
  <manifest>
    <item id="cover-img" href="cover.jpg" media-type="image/jpeg"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="nav"/></spine>
</package>"#;

/// Read the `<dc:title>` value from a parsed EPUB (the `epub` crate exposes
/// metadata by property name, not via a dedicated title accessor).
fn epub_title<R: std::io::Read + std::io::Seek>(doc: &EpubDoc<R>) -> Option<String> {
    doc.mdata("title").map(|m| m.value.trim().to_string())
}

/// Insert a `scan_roots` row pointing at `lib_dir` plus a `books` +
/// `book_files` (EPUB) row for `uuid`, so `book_file_path` resolves to
/// `lib_dir/<filename_stem>.epub` — real enough for `rewrite_blocking` to
/// open when the caller has also copied a fixture there (or left it
/// missing, to exercise the per-book failure branch). `pub(crate)` so
/// `worker::tests` can reuse it for the `Task::RewriteAllEpubs` bake-error
/// tests rather than duplicating the same three inserts.
pub(crate) async fn seed_epub_row(
    pool: &sqlx::SqlitePool,
    lib_dir: &std::path::Path,
    uuid: &str,
    title: &str,
    filename_stem: &str,
) -> i64 {
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(lib_dir.to_string_lossy().to_string())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?)")
            .bind(uuid)
            .bind(lib_id)
            .bind(title)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(filename_stem)
    .execute(pool)
    .await
    .unwrap();
    book_id
}
