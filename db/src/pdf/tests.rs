use std::path::{Path, PathBuf};

use super::*;
use crate::ebook::{parse_ebook_targets, ParseTarget, ScanOptions};
use crate::test_support::{build_test_pdf, make_test_dir, solid_color_png, EnvVarGuard, TestPdf};

fn write_pdf(dir: &Path, name: &str, spec: &TestPdf<'_>) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, build_test_pdf(spec)).unwrap();
    path
}

fn three_page_spec<'a>() -> TestPdf<'a> {
    TestPdf {
        pages: &["Chapter one text", "Second page", "Third"],
        title: Some("The Test Book"),
        author: Some("Ada Lovelace, Charles Babbage"),
        keywords: Some("Science; History, science"),
        ..Default::default()
    }
}

#[test]
fn extract_pdf_reads_info_dict_page_count_and_renders_a_cover() {
    let dir = make_test_dir("pdf_extract_happy");
    let path = write_pdf(&dir, "book.pdf", &three_page_spec());

    let book = extract_pdf(&path, "book.pdf".into(), &ScanOptions::default());

    let m = &book.metadata;
    assert_eq!(m.error, None);
    assert_eq!(m.title.as_deref(), Some("The Test Book"));
    let names: Vec<&str> = m.creators.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Ada Lovelace", "Charles Babbage"]);
    assert_eq!(
        m.subjects,
        vec!["Science".to_string(), "History".to_string()]
    );
    assert_eq!(m.page_count, Some(3));
    assert_eq!(book.word_count, Some(6), "three + two + one words");
    let (mime, bytes) = book.cover.expect("page 1 renders as the cover");
    assert_eq!(mime, "image/jpeg");
    assert!(bytes.starts_with(&[0xFF, 0xD8]), "JPEG magic");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn extract_pdf_falls_back_to_the_filename_stem_without_a_title() {
    let dir = make_test_dir("pdf_extract_stem");
    let spec = TestPdf {
        pages: &["Some text"],
        ..Default::default()
    };
    let path = write_pdf(&dir, "Field Notes 2024.pdf", &spec);

    let book = extract_pdf(
        &path,
        "Field Notes 2024.pdf".into(),
        &ScanOptions::default(),
    );

    assert_eq!(book.metadata.title.as_deref(), Some("Field Notes 2024"));
    assert!(book.metadata.creators.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn extract_pdf_surfaces_a_non_pdf_as_an_error_row() {
    let dir = make_test_dir("pdf_extract_garbage");
    let path = dir.join("bad.pdf");
    std::fs::write(&path, b"%PDF-1.4\nthis is not a real document").unwrap();

    let book = extract_pdf(&path, "bad.pdf".into(), &ScanOptions::default());

    let err = book.metadata.error.expect("malformed file is an error row");
    assert!(err.starts_with("could not open pdf:"), "{err}");
    assert_eq!(book.cover, None);
    assert_eq!(book.metadata.page_count, None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn extract_pdf_prefers_a_sidecar_cover_over_the_render() {
    let dir = make_test_dir("pdf_extract_sidecar");
    let path = write_pdf(&dir, "book.pdf", &three_page_spec());
    let sidecar = solid_color_png(10, 20, 30, 4, 4);
    std::fs::write(dir.join("book.png"), &sidecar).unwrap();

    let book = extract_pdf(&path, "book.pdf".into(), &ScanOptions::default());

    let (mime, bytes) = book.cover.expect("sidecar cover");
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, sidecar);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn extract_pdf_skips_text_past_the_size_cap_but_keeps_metadata() {
    let _cap = EnvVarGuard::set("OMNIBUS_PDF_TEXT_MAX_BYTES", Some("16"));
    let dir = make_test_dir("pdf_extract_cap");
    let path = write_pdf(&dir, "book.pdf", &three_page_spec());

    let book = extract_pdf(&path, "book.pdf".into(), &ScanOptions::default());

    assert_eq!(book.metadata.error, None);
    assert_eq!(book.metadata.title.as_deref(), Some("The Test Book"));
    assert_eq!(book.metadata.page_count, Some(3));
    assert!(
        book.cover.is_some(),
        "the lazy parser still renders the cover"
    );
    assert_eq!(book.word_count, None, "text is skipped past the cap");
    assert!(
        extract_structure(&path).is_err(),
        "structure honors the cap"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_ebook_targets_routes_pdf_targets_to_the_pdf_parser() {
    let dir = make_test_dir("pdf_dispatch");
    let path = write_pdf(&dir, "routed.pdf", &three_page_spec());
    let targets = vec![ParseTarget {
        filename: "routed.pdf".into(),
        absolute: path,
        mtime_epoch: 7,
        size_bytes: 9,
    }];

    let books = parse_ebook_targets(targets, ScanOptions::default());

    assert_eq!(books.len(), 1);
    assert_eq!(books[0].metadata.title.as_deref(), Some("The Test Book"));
    assert_eq!((books[0].mtime_epoch, books[0].size_bytes), (7, 9));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn page_texts_returns_one_entry_per_page_with_empty_pages_kept() {
    let dir = make_test_dir("pdf_page_texts");
    let spec = TestPdf {
        pages: &["Alpha line one\nAlpha line two", "", "Gamma"],
        ..Default::default()
    };
    let path = write_pdf(&dir, "book.pdf", &spec);

    let pages = page_texts(&path).unwrap();

    assert_eq!(
        pages,
        vec![
            "Alpha line one\nAlpha line two".to_string(),
            String::new(),
            "Gamma".to_string()
        ]
    );
    assert_eq!(page_count(&path).unwrap(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn page_text_returns_none_out_of_range() {
    let dir = make_test_dir("pdf_page_text");
    let spec = TestPdf {
        pages: &["Only page"],
        ..Default::default()
    };
    let path = write_pdf(&dir, "book.pdf", &spec);

    assert_eq!(page_text(&path, 0).unwrap().as_deref(), Some("Only page"));
    assert_eq!(page_text(&path, 1).unwrap(), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn extract_structure_weighs_pages_by_text_and_maps_the_outline() {
    let dir = make_test_dir("pdf_structure");
    let spec = TestPdf {
        pages: &["Intro text here", "", "Chapter two body"],
        outline: &[("Part I", 0), ("Part II", 2), ("", 1), ("Beyond", 9)],
        ..Default::default()
    };
    let path = write_pdf(&dir, "book.pdf", &spec);

    let structure = extract_structure(&path).unwrap().expect("pages exist");

    let weights: Vec<i64> = structure.spine.iter().map(|s| s.visible_chars).collect();
    assert_eq!(
        weights,
        [15, 1, 16],
        "a textless page still weighs one unit"
    );
    assert_eq!(structure.spine[1].href, "page:1");
    let chapters: Vec<(i64, &str, i64, i64)> = structure
        .chapters
        .iter()
        .map(|c| (c.ordinal, c.title.as_str(), c.spine_index, c.start_chars))
        .collect();
    // The blank title is dropped; "Beyond" points past the last page and is
    // clamped by the builder onto page 2, so it lands there.
    assert_eq!(
        chapters,
        [
            (0, "Part I", 0, 0),
            (1, "Part II", 2, 16),
            (2, "Beyond", 2, 16)
        ]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn decode_pdf_string_handles_utf16_bom_utf8_bom_and_latin1() {
    assert_eq!(
        decode_pdf_string(&[0xFE, 0xFF, 0x00, 0x48, 0x00, 0xE9]).as_deref(),
        Some("Hé")
    );
    assert_eq!(
        decode_pdf_string(&[0xEF, 0xBB, 0xBF, b'O', b'k']).as_deref(),
        Some("Ok")
    );
    assert_eq!(
        decode_pdf_string(&[b'C', b'a', b'f', 0xE9]).as_deref(),
        Some("Café")
    );
    assert_eq!(decode_pdf_string(b"   "), None);
}

#[test]
fn is_pdf_path_matches_the_extension_case_insensitively() {
    assert!(is_pdf_path(Path::new("a/b.pdf")));
    assert!(is_pdf_path(Path::new("a/b.PDF")));
    assert!(!is_pdf_path(Path::new("a/b.epub")));
    assert!(!is_pdf_path(Path::new("pdf")));
}

#[test]
fn ebook_extensions_mirror_ebook_formats() {
    let mut exts: Vec<String> = crate::ebook::EBOOK_EXTENSIONS
        .iter()
        .map(|e| e.to_ascii_uppercase())
        .collect();
    let mut formats: Vec<String> = crate::ebook::EBOOK_FORMATS
        .iter()
        .map(|f| f.to_string())
        .collect();
    exts.sort();
    formats.sort();
    assert_eq!(
        exts, formats,
        "settings-page count vs indexed formats drifted"
    );
}
