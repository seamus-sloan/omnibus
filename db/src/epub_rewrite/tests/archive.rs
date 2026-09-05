//! The archive rewrite on real EPUB bytes: baking the title and swapping
//! the cover, metadata without a cover override, the decompression size
//! cap, and clean errors for a non-zip file or a truncated central
//! directory.

use std::io::{Read, Write};

use epub::doc::EpubDoc;
use omnibus_shared::EbookMetadata;

use super::{epub_title, png, SAMPLE_OPF};
use crate::ebook::test_support::copy_fixture_into;
use crate::test_support::CoversTempDir;

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
    super::super::rewrite_blocking(&src, &dst, &book).unwrap();

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
    super::super::rewrite_blocking(&src, &dst, &book).unwrap();
    let doc = EpubDoc::new(&dst).unwrap();
    assert_eq!(epub_title(&doc).as_deref(), Some("Text Only Edit"));
}

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
    const OVER_CAP: u64 = super::super::archive::MAX_ENTRY_BYTES + 1;
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

    let err = super::super::archive::rewrite_archive(
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

    let err = super::super::archive::rewrite_archive(
        &src,
        &dst,
        "content.opf",
        |raw| Ok(raw.to_vec()),
        None,
    )
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

    let err = super::super::archive::rewrite_archive(
        &src,
        &dst,
        "content.opf",
        |raw| Ok(raw.to_vec()),
        None,
    )
    .unwrap_err();
    assert!(!err.to_string().is_empty(), "expected a descriptive error");
}
