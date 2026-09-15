//! PDF metadata extraction (server-only). Sibling to [`crate::ebook`]'s EPUB
//! parser and [`crate::comic`]: the pure-Rust `hayro` parser supplies the page
//! count, the Info dict, and a page-1 cover raster, and `pdf-extract`
//! supplies per-page text and the outline. Produces the [`IndexedBook`] shape
//! the sync writer consumes; the text half also backs the content index, the
//! structure tables, and the chapter-text read.

mod cover;
mod meta;
mod structure;
mod text;

#[cfg(test)]
mod tests;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use hayro::hayro_syntax::Pdf;
use omnibus_shared::{Contributor, EbookMetadata};

pub use cover::extract_cover;
pub use meta::{decode_pdf_string, PdfInfo};
pub use structure::extract_structure;
pub use text::{page_count, page_text, page_texts};

use crate::ebook::{extract_accent, resolve_cover_with, IndexedBook, ScanOptions};

/// `true` when `path` is a PDF by extension — the dispatch test
/// [`crate::ebook::parse_ebook_targets`] uses to route a scan target here.
pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Default for [`text_max_bytes`]: 128 MiB. The text extractor loads the
/// whole document (several times the file's size in memory), so a scanned
/// multi-hundred-MB PDF is skipped for text and outline; the lazy `hayro`
/// path still supplies metadata, page count, and the cover for any size.
const DEFAULT_TEXT_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// Largest file the text/outline extractor will load, from
/// `OMNIBUS_PDF_TEXT_MAX_BYTES` (bytes) else [`DEFAULT_TEXT_MAX_BYTES`].
pub fn text_max_bytes() -> u64 {
    std::env::var("OMNIBUS_PDF_TEXT_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_TEXT_MAX_BYTES)
}

/// Extract the [`IndexedBook`] for a single PDF. Mirrors the EPUB parser's
/// failure contract: an unparseable file is logged and surfaced as a
/// per-book error row — never a panic or an aborted scan. The parsers run
/// under `catch_unwind` because the scan's blocking phase has no other
/// guard, and a malformed font table must not take a library down with it.
pub fn extract_pdf(path: &Path, filename: String, opts: &ScanOptions) -> IndexedBook {
    match read_pdf(path) {
        Ok(parsed) => indexed_book_from(parsed, path, filename, opts),
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "skipping malformed PDF");
            IndexedBook {
                metadata: EbookMetadata {
                    filename,
                    error: Some(format!("could not open pdf: {e}")),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
                word_count: None,
            }
        }
    }
}

/// Everything one pass over the file yields: the page count, the Info dict,
/// the rendered first page (the cover candidate), and the word count.
struct ParsedPdf {
    page_count: usize,
    info: PdfInfo,
    first_page: Option<(String, Vec<u8>)>,
    word_count: Option<i64>,
}

fn read_pdf(path: &Path) -> anyhow::Result<ParsedPdf> {
    let bytes = std::fs::read(path)?;
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new(bytes)))
        .map_err(|_| anyhow::anyhow!("pdf parser panicked"))?
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let page_count = pdf.pages().len();
    if page_count == 0 {
        anyhow::bail!("no pages in document");
    }
    let info = meta::read_info(&pdf);
    let first_page = cover::render_first_page(&pdf);
    drop(pdf);
    let word_count = text::word_count(path);
    Ok(ParsedPdf {
        page_count,
        info,
        first_page,
        word_count,
    })
}

/// Project a [`ParsedPdf`] into the writer's [`IndexedBook`] shape. Info-dict
/// fields win; a missing title falls back to the filename stem, same as the
/// comic and audiobook parsers.
fn indexed_book_from(
    parsed: ParsedPdf,
    path: &Path,
    filename: String,
    opts: &ScanOptions,
) -> IndexedBook {
    let title = parsed
        .info
        .title
        .clone()
        .unwrap_or_else(|| filename_stem(&filename));
    let creators = parsed
        .info
        .author
        .as_deref()
        .map(contributors_from_author)
        .unwrap_or_default();
    let cover = resolve_cover_with(path, opts, || parsed.first_page);
    let accent = cover
        .as_ref()
        .and_then(|(_mime, bytes)| extract_accent(bytes));
    IndexedBook {
        metadata: EbookMetadata {
            filename,
            title: Some(title),
            creators,
            description: parsed.info.subject,
            subjects: parsed.info.keywords,
            accent,
            page_count: Some(parsed.page_count as i64),
            ..Default::default()
        },
        cover,
        // Stat values get overwritten by `parse_ebook_targets` before the
        // writer sees this struct — same pattern as the EPUB parser.
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: parsed.word_count,
    }
}

/// An Info-dict `/Author` is one string; several names arrive joined by
/// commas, semicolons, or " and ", so split on those the way the comic
/// parser splits `ComicInfo.xml`'s writer list.
fn contributors_from_author(author: &str) -> Vec<Contributor> {
    author
        .split([',', ';'])
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Contributor {
            name: name.to_string(),
            ..Default::default()
        })
        .collect()
}

fn filename_stem(filename: &str) -> String {
    Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| filename.to_string())
}
