//! Tests for the OPF 2.0 sidecar writer: the pure `render_opf` renderer
//! (exact-string output + escaping) and the `export_opf` orchestration
//! (snapshot, override merge, backup, cover copy, error variants).

use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};

use super::*;
use crate::books::get_book;
use crate::metadata_overrides::{merge_metadata_overrides, upsert_metadata_overrides};
use crate::pool::init_db;
use crate::test_support::{seed_epub_book_at, CoversTempDir};

// ---------------------------------------------------------------------------
// render_opf — pure renderer
// ---------------------------------------------------------------------------

#[test]
fn render_opf_emits_full_opf2_document_for_complete_metadata() {
    let book = EbookMetadata {
        filename: "book.epub".into(),
        title: Some("The Title".into()),
        description: Some("A short blurb.".into()),
        publisher: Some("Acme Press".into()),
        published: Some("2021-05-01".into()),
        language: Some("en".into()),
        creators: vec![
            Contributor {
                name: "Ada Author".into(),
                role: Some("aut".into()),
                file_as: Some("Author, Ada".into()),
                id: None,
            },
            Contributor {
                name: "Bob Illustrator".into(),
                role: None,
                file_as: None,
                id: None,
            },
        ],
        subjects: vec!["Fiction".into(), "Adventure".into()],
        identifiers: vec![Identifier {
            value: "OL123M".into(),
            scheme: Some("OpenLibrary".into()),
        }],
        isbn13: Some("9781234567897".into()),
        series: Some("The Saga".into()),
        series_index: Some("2".into()),
        unique_identifier: Some("uuid-1".into()),
        ..Default::default()
    };

    let expected = "\
<?xml version=\"1.0\" encoding=\"utf-8\"?>
<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uuid_id\" version=\"2.0\">
  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">
    <dc:identifier opf:scheme=\"uuid\" id=\"uuid_id\">uuid-1</dc:identifier>
    <dc:identifier opf:scheme=\"OpenLibrary\">OL123M</dc:identifier>
    <dc:identifier opf:scheme=\"ISBN\">9781234567897</dc:identifier>
    <dc:title>The Title</dc:title>
    <dc:creator opf:role=\"aut\" opf:file-as=\"Author, Ada\">Ada Author</dc:creator>
    <dc:creator>Bob Illustrator</dc:creator>
    <dc:description>A short blurb.</dc:description>
    <dc:publisher>Acme Press</dc:publisher>
    <dc:date>2021-05-01</dc:date>
    <dc:language>en</dc:language>
    <dc:subject>Fiction</dc:subject>
    <dc:subject>Adventure</dc:subject>
    <meta name=\"calibre:series\" content=\"The Saga\"/>
    <meta name=\"calibre:series_index\" content=\"2\"/>
  </metadata>
</package>
";
    assert_eq!(render_opf(&book), expected);
}

#[test]
fn render_opf_omits_absent_optional_elements_and_falls_back_to_filename_title() {
    let book = EbookMetadata {
        filename: "fallback.epub".into(),
        unique_identifier: Some("uuid-1".into()),
        ..Default::default()
    };

    let expected = "\
<?xml version=\"1.0\" encoding=\"utf-8\"?>
<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uuid_id\" version=\"2.0\">
  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">
    <dc:identifier opf:scheme=\"uuid\" id=\"uuid_id\">uuid-1</dc:identifier>
    <dc:title>fallback.epub</dc:title>
  </metadata>
</package>
";
    assert_eq!(render_opf(&book), expected);
}

#[test]
fn render_opf_uses_filename_as_package_identifier_when_uuid_absent() {
    // `render_opf` is pub and `unique_identifier` is `Option`; a directly
    // built book with `None` must still emit the `id="uuid_id"` element the
    // package's `unique-identifier` references, or the OPF is invalid.
    let book = EbookMetadata {
        filename: "no-uuid.epub".into(),
        title: Some("T".into()),
        ..Default::default()
    };

    let xml = render_opf(&book);
    assert!(xml.contains("unique-identifier=\"uuid_id\""));
    assert!(xml.contains(
        "<dc:identifier opf:scheme=\"uuid\" id=\"uuid_id\">no-uuid.epub</dc:identifier>"
    ));
}

#[test]
fn render_opf_escapes_xml_special_characters_in_text_and_attributes() {
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("R&D <\"Beta\">".into()),
        creators: vec![Contributor {
            name: "A & B".into(),
            role: None,
            file_as: Some("O'Neil & \"Co\"".into()),
            id: None,
        }],
        unique_identifier: Some("uuid-1".into()),
        ..Default::default()
    };

    let xml = render_opf(&book);
    assert!(xml.contains("<dc:title>R&amp;D &lt;&quot;Beta&quot;&gt;</dc:title>"));
    assert!(xml.contains("opf:file-as=\"O&apos;Neil &amp; &quot;Co&quot;\""));
    assert!(xml.contains(">A &amp; B</dc:creator>"));
}

#[test]
fn render_opf_strips_xml_illegal_control_chars_while_keeping_tab_lf_cr() {
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("Stray\u{1}Control".into()),
        description: Some("Tab\tKept\nLF\rCR\u{c}Stripped".into()),
        creators: vec![Contributor {
            name: "Author\u{0}Name".into(),
            role: None,
            file_as: None,
            id: None,
        }],
        unique_identifier: Some("uuid-1".into()),
        ..Default::default()
    };

    let xml = render_opf(&book);
    assert!(xml.contains("<dc:title>StrayControl</dc:title>"));
    assert!(xml.contains("<dc:description>Tab\tKept\nLF\rCRStripped</dc:description>"));
    assert!(xml.contains(">AuthorName</dc:creator>"));
    // The document as a whole stays well-formed: no illegal control chars
    // survive anywhere in the output.
    assert!(!xml.chars().any(is_xml_illegal_control_char));
}

#[test]
fn render_opf_skips_duplicate_isbn_identifier_when_already_scanned() {
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("T".into()),
        identifiers: vec![Identifier {
            value: "9781234567897".into(),
            scheme: Some("ISBN".into()),
        }],
        isbn13: Some("9781234567897".into()),
        unique_identifier: Some("uuid-1".into()),
        ..Default::default()
    };

    let count = render_opf(&book).matches("9781234567897").count();
    assert_eq!(count, 1);
}

// ---------------------------------------------------------------------------
// export_opf — orchestration
// ---------------------------------------------------------------------------

async fn seed_user(pool: &sqlx::SqlitePool) -> i64 {
    crate::auth::create_user(pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn export_opf_writes_snapshot_of_scanned_metadata_when_no_overrides() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let (book_id, _uuid) = seed_epub_book_at(&pool, lib.path()).await;

    let result = export_opf(&pool, book_id).await.unwrap();
    assert!(!result.backed_up);

    let written = std::fs::read_to_string(lib.path().join("sub").join("metadata.opf")).unwrap();
    let expected = render_opf(&get_book(&pool, book_id).await.unwrap().unwrap());
    assert_eq!(written, expected);
    assert!(written.contains("<dc:title>Title</dc:title>"));
}

#[tokio::test]
async fn export_opf_merges_overrides_into_written_snapshot() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = seed_user(&pool).await;
    let lib = tempfile::tempdir().unwrap();
    let (book_id, uuid) = seed_epub_book_at(&pool, lib.path()).await;

    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Edited Title".into()),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    export_opf(&pool, book_id).await.unwrap();
    let written = std::fs::read_to_string(lib.path().join("sub").join("metadata.opf")).unwrap();
    assert!(written.contains("<dc:title>Edited Title</dc:title>"));
}

#[tokio::test]
async fn export_opf_renames_existing_sidecar_to_bak_and_reports_backed_up() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let (book_id, _uuid) = seed_epub_book_at(&pool, lib.path()).await;
    let dir = lib.path().join("sub");
    std::fs::write(dir.join("metadata.opf"), b"OLD SIDECAR").unwrap();

    let first = export_opf(&pool, book_id).await.unwrap();
    assert!(first.backed_up);
    assert_eq!(
        std::fs::read_to_string(dir.join("metadata.opf.bak")).unwrap(),
        "OLD SIDECAR"
    );
    let fresh = std::fs::read_to_string(dir.join("metadata.opf")).unwrap();
    assert!(fresh.starts_with("<?xml"));

    // A second export overwrites the prior .bak with the first export's output.
    let second = export_opf(&pool, book_id).await.unwrap();
    assert!(second.backed_up);
    assert_eq!(
        std::fs::read_to_string(dir.join("metadata.opf.bak")).unwrap(),
        fresh
    );
}

#[tokio::test]
async fn export_opf_copies_jpeg_override_cover_verbatim() {
    let _covers = CoversTempDir::new("opf-jpeg");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = seed_user(&pool).await;
    let lib = tempfile::tempdir().unwrap();
    let (book_id, uuid) = seed_epub_book_at(&pool, lib.path()).await;

    // A minimal JPEG the writer must copy byte-for-byte (no re-encode).
    std::fs::create_dir_all(crate::covers::covers_dir()).unwrap();
    let jpeg = encode_test_image(image::ImageFormat::Jpeg);
    std::fs::write(
        crate::covers::covers_dir().join(format!("override-{uuid}.jpg")),
        &jpeg,
    )
    .unwrap();
    upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
        .await
        .unwrap();

    export_opf(&pool, book_id).await.unwrap();
    let copied = std::fs::read(lib.path().join("sub").join("cover.jpg")).unwrap();
    assert_eq!(copied, jpeg);
}

#[tokio::test]
async fn export_opf_reencodes_png_override_cover_to_jpeg() {
    let _covers = CoversTempDir::new("opf-png");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = seed_user(&pool).await;
    let lib = tempfile::tempdir().unwrap();
    let (book_id, uuid) = seed_epub_book_at(&pool, lib.path()).await;

    std::fs::create_dir_all(crate::covers::covers_dir()).unwrap();
    let png = encode_test_image(image::ImageFormat::Png);
    std::fs::write(
        crate::covers::covers_dir().join(format!("override-{uuid}.png")),
        &png,
    )
    .unwrap();
    upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
        .await
        .unwrap();

    export_opf(&pool, book_id).await.unwrap();
    let bytes = std::fs::read(lib.path().join("sub").join("cover.jpg")).unwrap();
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::Jpeg
    );
}

#[tokio::test]
async fn export_opf_returns_book_not_found_for_unknown_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let err = export_opf(&pool, 9999).await.unwrap_err();
    assert!(matches!(err, OpfExportError::BookNotFound(9999)));
}

#[tokio::test]
async fn export_opf_returns_no_epub_file_for_audiobook_only_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(lib.path().to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified)
         VALUES ('uuid-ab', 'sub/audio', 1, 'sub', 'Audio', '2000-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-ab'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'M4B', 'audio', 4, 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let err = export_opf(&pool, book_id).await.unwrap_err();
    assert!(matches!(err, OpfExportError::NoEpubFile(_)));
}

#[tokio::test]
async fn export_opf_returns_books_error_when_book_files_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let (book_id, _uuid) = seed_epub_book_at(&pool, lib.path()).await;
    // get_book's final step (get_book_files) queries book_files; dropping it
    // forces the crate::books::BooksError this exercises.
    sqlx::query("DROP TABLE book_files")
        .execute(&pool)
        .await
        .unwrap();

    let err = export_opf(&pool, book_id).await.unwrap_err();
    assert!(matches!(err, OpfExportError::Books(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn export_opf_returns_io_error_when_book_directory_is_not_writable() {
    use std::os::unix::fs::PermissionsExt;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let (book_id, _uuid) = seed_epub_book_at(&pool, lib.path()).await;
    let dir = lib.path().join("sub");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    // Skip if the chmod didn't take (e.g. running as root in some CI
    // containers), same guard as db/src/ebook/cover/tests.rs.
    if std::fs::write(dir.join("write_probe"), b"x").is_ok() {
        std::fs::remove_file(dir.join("write_probe")).ok();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let err = export_opf(&pool, book_id).await.unwrap_err();

    // Restore perms so the tempdir can clean itself up.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(matches!(err, OpfExportError::Io(_)));
}

#[tokio::test]
async fn write_sidecar_restores_backup_when_promotion_rename_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("metadata.opf"), b"ORIGINAL SIDECAR").unwrap();

    let err = write_sidecar_forcing_promote_failure(dir.path(), "<new/>")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Other);

    // The pre-existing sidecar must be restored, not left missing, and no
    // stray backup/temp file should remain.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("metadata.opf")).unwrap(),
        "ORIGINAL SIDECAR"
    );
    assert!(!dir.path().join("metadata.opf.bak").exists());
    assert!(!dir.path().join(".metadata.opf.tmp").exists());
}

/// Encode a 1×1 red image in `format` for the cover-copy tests.
fn encode_test_image(format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}
