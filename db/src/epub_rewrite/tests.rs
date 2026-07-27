//! Tests for the export-with-overrides EPUB rewrite (F5.8 #1372).

use std::io::{Cursor, Read, Write};

use epub::doc::EpubDoc;
use image::{DynamicImage, ImageFormat, RgbImage};
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use super::cover::encode_cover_for;
use super::opf::transform_opf;
use super::EpubRewriteError;
use crate::ebook::test_support::{copy_fixture_into, fixture};
use crate::test_support::{CoversTempDir, EnvVarGuard};

// --- helpers -----------------------------------------------------------

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

/// Effective metadata standing in for a fully-overridden book.
fn overridden_book() -> EbookMetadata {
    EbookMetadata {
        id: 1,
        filename: "book.epub".into(),
        title: Some("New Title".into()),
        creators: vec![Contributor {
            name: "New Author".into(),
            role: Some("aut".into()),
            file_as: Some("Author, New".into()),
            id: None,
        }],
        subjects: vec!["NewSubj".into()],
        series: Some("New Series".into()),
        series_index: Some("1".into()),
        language: Some("en".into()),
        ..Default::default()
    }
}

/// Read the `<dc:title>` value from a parsed EPUB (the `epub` crate exposes
/// metadata by property name, not via a dedicated title accessor).
fn epub_title<R: std::io::Read + std::io::Seek>(doc: &EpubDoc<R>) -> Option<String> {
    doc.mdata("title").map(|m| m.value.trim().to_string())
}

/// Assert `xml` parses to EOF without a quick-xml error (well-formedness).
fn assert_well_formed(xml: &[u8]) {
    let mut reader = quick_xml::reader::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("rewritten OPF is not well-formed: {e}"),
        }
        buf.clear();
    }
}

// --- transform_opf -----------------------------------------------------

#[test]
fn transform_opf_replaces_managed_fields_and_preserves_structure() {
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &overridden_book()).unwrap();
    let s = String::from_utf8(out.clone()).unwrap();
    assert_well_formed(&out);

    // Managed descriptive fields are replaced.
    assert!(s.contains("<dc:title>New Title</dc:title>"), "{s}");
    assert!(!s.contains("Original Title"), "{s}");
    assert!(s.contains("New Author"), "{s}");
    assert!(!s.contains("Original Author"), "{s}");
    assert!(s.contains("<dc:subject>NewSubj</dc:subject>"), "{s}");
    assert!(!s.contains("OldSubject"), "{s}");
    assert!(
        s.contains(r#"<meta name="calibre:series" content="New Series"/>"#),
        "{s}"
    );
    assert!(!s.contains("Old Series"), "{s}");
    assert!(
        s.contains(r#"<meta name="calibre:series_index" content="1"/>"#),
        "{s}"
    );

    // Package identity, cover pointer, refinements, manifest, and spine survive.
    assert!(s.contains(r#"unique-identifier="bookid""#), "{s}");
    assert!(s.contains("urn:uuid:ORIG-UUID-9f3"), "{s}");
    assert!(
        s.contains(r#"<meta name="cover" content="cover-img"/>"#),
        "{s}"
    );
    assert!(s.contains("dcterms:modified"), "{s}");
    assert!(
        s.contains(r#"<item id="cover-img" href="cover.jpg" media-type="image/jpeg"/>"#),
        "{s}"
    );
    assert!(s.contains(r#"<itemref idref="nav"/>"#), "{s}");
}

#[test]
fn transform_opf_escapes_values_and_falls_back_to_filename_for_empty_title() {
    let book = EbookMetadata {
        filename: "fallback.epub".into(),
        title: None,
        publisher: Some("A & B <Publishing>".into()),
        ..Default::default()
    };
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &book).unwrap();
    let s = String::from_utf8(out.clone()).unwrap();
    assert_well_formed(&out);
    assert!(s.contains("<dc:title>fallback.epub</dc:title>"), "{s}");
    assert!(
        s.contains("<dc:publisher>A &amp; B &lt;Publishing&gt;</dc:publisher>"),
        "{s}"
    );
}

#[test]
fn transform_opf_drops_managed_fields_when_effective_value_is_absent() {
    // An override that clears series/subjects: the originals must not linger.
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("Only Title".into()),
        ..Default::default()
    };
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &book).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(!s.contains("calibre:series"), "{s}");
    assert!(!s.contains("OldSubject"), "{s}");
    // But the cover pointer (an unmanaged meta) is still preserved.
    assert!(
        s.contains(r#"<meta name="cover" content="cover-img"/>"#),
        "{s}"
    );
}

#[test]
fn transform_opf_errors_when_dc_namespace_is_unbound() {
    // Metadata that never binds `xmlns:dc` — injecting `<dc:*>` would be invalid
    // XML, so the rewrite must fail and let the caller serve the source EPUB.
    let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata>
    <identifier id="id">book-1</identifier>
    <title>Original</title>
  </metadata>
  <manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="nav"/></spine>
</package>"#;
    let err = transform_opf(opf.as_bytes(), &overridden_book()).unwrap_err();
    assert!(
        err.to_string().contains("xmlns:dc"),
        "expected an unbound-dc error, got: {err}"
    );
}

// --- encode_cover_for --------------------------------------------------

#[test]
fn encode_cover_for_returns_bytes_unchanged_when_already_target_format() {
    let src = png(2, 2, [10, 20, 30]);
    let out = encode_cover_for("image/png", "image/png", src.clone()).unwrap();
    assert_eq!(out, src);
}

#[test]
fn encode_cover_for_transcodes_to_target_format() {
    let src = png(4, 4, [200, 100, 50]);
    let out = encode_cover_for("image/jpeg", "image/png", src).unwrap();
    assert_eq!(
        image::guess_format(&out).unwrap(),
        ImageFormat::Jpeg,
        "output should be JPEG"
    );
}

#[test]
fn encode_cover_for_returns_none_for_undecodable_override() {
    assert!(encode_cover_for("image/jpeg", "image/png", b"not an image".to_vec()).is_none());
}

#[test]
fn encode_cover_for_returns_none_for_unencodable_target_format() {
    let src = png(2, 2, [0, 0, 0]);
    assert!(encode_cover_for("image/svg+xml", "image/png", src).is_none());
}

// --- rewrite_blocking (real fixture, no DB) ----------------------------

#[test]
fn rewrite_blocking_bakes_title_and_swaps_cover_in_real_epub() {
    let tmp = tempfile::tempdir().unwrap();
    let src = copy_fixture_into("alpha.epub", tmp.path());
    let original_cover = EpubDoc::new(&src)
        .unwrap()
        .get_cover()
        .expect("alpha.epub has an embedded cover")
        .0;

    let uuid = "rewrite-blocking-uuid";
    let covers = CoversTempDir::new("epub_rewrite_blocking");
    std::fs::create_dir_all(&covers.path).unwrap();
    std::fs::write(
        covers.path.join(format!("override-{uuid}.png")),
        png(3, 3, [255, 0, 0]),
    )
    .unwrap();

    let book = EbookMetadata {
        id: 1,
        filename: "alpha.epub".into(),
        title: Some("Baked Title".into()),
        unique_identifier: Some(uuid.into()),
        has_cover_override: true,
        ..Default::default()
    };

    let dst = tmp.path().join("out.epub");
    super::rewrite_blocking(&src, &dst, &book).unwrap();

    let mut doc = EpubDoc::new(&dst).expect("rewritten epub is a valid container");
    assert_eq!(epub_title(&doc).as_deref(), Some("Baked Title"));
    assert!(!doc.spine.is_empty(), "spine preserved");

    let new_cover = doc.get_cover().expect("cover still present").0;
    assert_ne!(new_cover, original_cover, "cover bytes were swapped");
    assert_eq!(
        image::load_from_memory(&new_cover).unwrap().width(),
        3,
        "swapped cover is the 3x3 override"
    );
}

#[test]
fn rewrite_blocking_bakes_metadata_without_cover_override() {
    let tmp = tempfile::tempdir().unwrap();
    let src = copy_fixture_into("alpha.epub", tmp.path());
    let book = EbookMetadata {
        id: 1,
        filename: "alpha.epub".into(),
        title: Some("Text Only Edit".into()),
        has_cover_override: false,
        ..Default::default()
    };
    let dst = tmp.path().join("out.epub");
    super::rewrite_blocking(&src, &dst, &book).unwrap();
    let doc = EpubDoc::new(&dst).unwrap();
    assert_eq!(epub_title(&doc).as_deref(), Some("Text Only Edit"));
}

// --- rewrite_archive (zip-entry size bound, #1394) ---------------------

#[test]
fn rewrite_archive_errors_on_entry_that_decompresses_past_the_size_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("bomb.epub");
    let dst = tmp.path().join("out.epub");

    // Zip-bomb-style fixture: `huge.bin` decompresses to one byte past the
    // per-entry read cap (test builds use a much smaller `MAX_ENTRY_BYTES`
    // than production so this regression stays fast), but — being all zeros
    // — deflates to a few bytes on disk, exactly the "small compressed, huge
    // decompressed" shape a hostile EPUB upload could exploit. Streamed via
    // `io::repeat` so *building* the fixture never materializes the cap's
    // worth of bytes in memory either.
    const OVER_CAP: u64 = super::archive::MAX_ENTRY_BYTES + 1;
    let file = std::fs::File::create(&src).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    writer
        .start_file(
            "huge.bin",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    std::io::copy(&mut std::io::repeat(0).take(OVER_CAP), &mut writer).unwrap();
    writer.finish().unwrap();

    let err = super::archive::rewrite_archive(
        &src,
        &dst,
        "nonexistent.opf",
        |raw| Ok(raw.to_vec()),
        None,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("exceeds") && err.to_string().contains("byte cap"),
        "expected a size-cap error, got: {err}"
    );
}

#[test]
fn rewrite_archive_returns_a_clean_err_for_a_file_that_is_not_a_zip_container() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("not-a-zip.epub");
    let dst = tmp.path().join("out.epub");

    // No zip signature at all — the shape a completely garbled/non-EPUB
    // upload would take. Must surface as a plain `Err`, never a panic.
    std::fs::write(&src, b"this file is not a zip archive, just plain text").unwrap();

    let err =
        super::archive::rewrite_archive(&src, &dst, "content.opf", |raw| Ok(raw.to_vec()), None)
            .unwrap_err();
    assert!(
        err.to_string().contains("read source epub as zip"),
        "expected a zip-parse error, got: {err}"
    );
}

#[test]
fn rewrite_archive_returns_a_clean_err_for_a_zip_truncated_mid_central_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let full = tmp.path().join("full.epub");
    let src = tmp.path().join("truncated.epub");
    let dst = tmp.path().join("out.epub");

    // A well-formed small zip, then chopped in half — local file headers and
    // data may still be intact, but the end-of-central-directory record (at
    // the tail) is gone. Models a partially-downloaded or disk-corrupted
    // EPUB rather than a file that was never a zip at all.
    {
        let file = std::fs::File::create(&full).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "mimetype",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"application/epub+zip").unwrap();
        writer
            .start_file("content.opf", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(SAMPLE_OPF.as_bytes()).unwrap();
        writer.finish().unwrap();
    }
    let full_bytes = std::fs::read(&full).unwrap();
    std::fs::write(&src, &full_bytes[..full_bytes.len() / 2]).unwrap();

    let err =
        super::archive::rewrite_archive(&src, &dst, "content.opf", |raw| Ok(raw.to_vec()), None)
            .unwrap_err();
    assert!(!err.to_string().is_empty(), "expected a descriptive error");
}

// --- rewritten_epub_path (DB-integrated) -------------------------------

#[tokio::test]
async fn rewritten_epub_path_returns_none_without_overrides() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "wok.epub", "The Way of Kings", "Sanderson")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    let src = fixture("alpha.epub");
    let out = super::rewritten_epub_path(&pool, id, &src).await.unwrap();
    assert!(out.is_none(), "no overrides → serve source, no rewrite");
}

#[tokio::test]
async fn rewritten_epub_path_bakes_title_override_into_export() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "wok.epub", "The Way of Kings", "Sanderson")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    let overrides = omnibus_shared::MetadataOverrides {
        title: Some("Stormlight #1".into()),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, user_id)
        .await
        .unwrap();

    let src = fixture("alpha.epub");
    let out = super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap()
        .expect("override present → rewritten export");

    let doc = EpubDoc::new(&out).unwrap();
    assert_eq!(epub_title(&doc).as_deref(), Some("Stormlight #1"));

    // Second call is idempotent — returns the same cached path.
    let again = super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, out);
}

#[tokio::test]
async fn is_stale_returns_true_when_export_missing() {
    assert!(super::is_stale(std::path::Path::new("/nonexistent/x.epub"), 0).await);
}

/// #1395: once a book's last override clears, `rewritten_epub_path` must
/// remove any cache file left behind rather than leaving it orphaned. This
/// exercises the belt-and-suspenders cleanup on the early-return path
/// directly — the eager cleanup in `metadata_overrides::delete_metadata_overrides`
/// / `clear_cover_override` is covered separately in that module's tests.
#[tokio::test]
async fn rewritten_epub_path_removes_a_leftover_cache_file_when_no_override_is_active() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "wok.epub", "The Way of Kings", "Sanderson")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    // Simulate an orphan left behind by a pre-#1395 build: a cache file with
    // no override active to justify it.
    let cache_path = super::export_epub_path(id);
    std::fs::write(&cache_path, b"orphaned rewritten epub").unwrap();

    let src = fixture("alpha.epub");
    let out = super::rewritten_epub_path(&pool, id, &src).await.unwrap();

    assert!(out.is_none(), "no overrides → serve source, no rewrite");
    assert!(
        !cache_path.exists(),
        "the orphaned cache file must be cleaned up rather than left on disk"
    );
}

// --- rewritten_epub_path error variants (#1396) ------------------------

#[tokio::test]
async fn rewritten_epub_path_returns_book_not_found_for_unknown_book_id() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let src = fixture("alpha.epub");

    let err = super::rewritten_epub_path(&pool, 9999, &src)
        .await
        .unwrap_err();
    assert!(matches!(err, EpubRewriteError::BookNotFound(9999)));
}

#[tokio::test]
async fn rewritten_epub_path_returns_books_error_when_pool_is_closed() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "closed-pool.epub", "Closed Pool", "Author")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();
    pool.close().await;

    let src = fixture("alpha.epub");
    let err = super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap_err();
    assert!(matches!(err, EpubRewriteError::Books(_)));
}

#[tokio::test]
async fn rewritten_epub_path_returns_failed_when_the_source_epub_is_missing() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(
        &pool,
        "missing-source.epub",
        "Missing Source",
        "Author",
    )
    .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    let overrides = MetadataOverrides {
        title: Some("New Title".into()),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, user_id)
        .await
        .unwrap();

    // An override is active, so `rewritten_epub_path` attempts the rewrite —
    // but `source` doesn't exist on disk, so opening it as an epub fails and
    // the anyhow error collapses into `Failed` per the module's doc comment.
    let src = export.path().join("does-not-exist.epub");
    let err = super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap_err();
    assert!(matches!(err, EpubRewriteError::Failed(_)));
}

/// `EpubRewriteError::Io` is declared for completeness (mirroring the
/// sibling `opf_export::OpfExportError`) but every filesystem failure inside
/// `rewritten_epub_path` is deliberately routed through `anyhow` context
/// first — per the enum's own doc comment, "every foreign-system failure...
/// collapses into `Failed`" — so `Io` is never constructed by that function
/// today. This exercises the `#[from] std::io::Error` conversion directly
/// so the variant still has a passing test and a regression catches it if a
/// future caller starts constructing it.
#[test]
fn epub_rewrite_error_io_variant_wraps_a_source_io_error_via_from() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
    let err: EpubRewriteError = io_err.into();
    assert!(matches!(err, EpubRewriteError::Io(_)));
}

// --- cap_bytes / evict_if_over_cap / invalidate_export_epub_cache -----

// Note: `EnvVarGuard` holds the process-wide `ENV_LOCK` for its lifetime and
// that mutex is not reentrant, so each test below keeps at most one guard
// alive at a time — two in one scope self-deadlocks the test binary.

#[test]
fn cap_bytes_falls_back_to_default_when_env_is_unset() {
    let _env = EnvVarGuard::set("OMNIBUS_EXPORT_EPUB_CAP_BYTES", None);
    assert_eq!(super::cap_bytes(), 5 * 1024 * 1024 * 1024);
}

#[test]
fn cap_bytes_falls_back_to_default_when_env_is_unparseable() {
    let _env = EnvVarGuard::set("OMNIBUS_EXPORT_EPUB_CAP_BYTES", Some("not-a-number"));
    assert_eq!(super::cap_bytes(), 5 * 1024 * 1024 * 1024);
}

#[test]
fn cap_bytes_reads_the_configured_env_var() {
    let _env = EnvVarGuard::set("OMNIBUS_EXPORT_EPUB_CAP_BYTES", Some("1024"));
    assert_eq!(super::cap_bytes(), 1024);
}

#[test]
fn invalidate_export_epub_cache_removes_an_existing_file_and_is_a_noop_when_absent() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let path = super::export_epub_path(42);
    std::fs::write(&path, b"cached").unwrap();
    assert!(path.exists());

    super::invalidate_export_epub_cache(42);
    assert!(!path.exists());

    // A second call with nothing left to remove must not panic or error.
    super::invalidate_export_epub_cache(42);
}

#[test]
fn evict_if_over_cap_removes_oldest_entries_until_under_the_cap() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    // Three 10-byte entries, oldest (book 1) to newest (book 3).
    for (book_id, age_secs) in [(1, 30), (2, 20), (3, 10)] {
        let path = super::export_epub_path(book_id);
        std::fs::write(&path, b"0123456789").unwrap();
        let mtime = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        let file = std::fs::File::open(&path).unwrap();
        file.set_modified(mtime).unwrap();
    }

    // Cap fits only the newest two entries (20 bytes).
    super::evict_if_over_cap(20).unwrap();

    assert!(
        !super::export_epub_path(1).exists(),
        "oldest entry should have been evicted"
    );
    assert!(super::export_epub_path(2).exists());
    assert!(super::export_epub_path(3).exists());
}

#[test]
fn evict_if_over_cap_ignores_in_flight_temp_files() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    // A temp file mid-rewrite (dot-prefixed, per `rewritten_epub_path`'s
    // naming scheme) must not be counted or deleted by eviction.
    let tmp = export.path().join(".7.12345.0.tmp.epub");
    std::fs::write(&tmp, b"in-flight").unwrap();

    super::evict_if_over_cap(0).unwrap();

    assert!(tmp.exists(), "in-flight temp file must survive eviction");
}

#[test]
fn evict_if_over_cap_is_a_noop_when_the_export_dir_does_not_exist() {
    let export = tempfile::tempdir().unwrap();
    let missing = export.path().join("does-not-exist");
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(missing.as_os_str()));

    super::evict_if_over_cap(0).unwrap();
}
