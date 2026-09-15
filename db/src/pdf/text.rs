//! Per-page plain text via `pdf-extract`, the pure-Rust extractor: one string
//! per page, addressed by 0-based page index — the PDF's "spine". Quality is
//! advisory: ligature-heavy and CJK text can come out fused or missing, and a
//! scanned page has no text at all. Every page runs under `catch_unwind`
//! because the extractor's font parsers panic on some real-world embedded
//! fonts, and one bad page must cost that page, not the book.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use hayro::hayro_syntax::Pdf;
use pdf_extract::{output_doc_page, Document, PlainTextOutput};

/// Open the document for text/outline work, refusing files past
/// [`super::text_max_bytes`] — the extractor loads the whole file into
/// memory several times over.
pub(super) fn load_document(path: &Path) -> anyhow::Result<Document> {
    let size = std::fs::metadata(path)?.len();
    let cap = super::text_max_bytes();
    if size > cap {
        anyhow::bail!("{size} bytes exceeds the {cap}-byte text extraction cap");
    }
    catch_unwind(AssertUnwindSafe(|| Document::load(path)))
        .map_err(|_| anyhow::anyhow!("pdf text parser panicked"))?
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The document's page count, from the lazy parser (no size cap: it reads
/// the page tree, not the page contents).
pub fn page_count(path: &Path) -> anyhow::Result<usize> {
    let bytes = std::fs::read(path)?;
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new(bytes)))
        .map_err(|_| anyhow::anyhow!("pdf parser panicked"))?
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(pdf.pages().len())
}

/// Every page's text, in page order; an unreadable or textless page is an
/// empty string so indices stay aligned with the page tree.
pub fn page_texts(path: &Path) -> anyhow::Result<Vec<String>> {
    let doc = load_document(path)?;
    let count = doc.get_pages().len();
    Ok((1..=count as u32).map(|p| page_text_of(&doc, p)).collect())
}

/// One page's text by 0-based index; `Ok(None)` when out of range.
pub fn page_text(path: &Path, index: usize) -> anyhow::Result<Option<String>> {
    let doc = load_document(path)?;
    let count = doc.get_pages().len();
    if index >= count {
        return Ok(None);
    }
    Ok(Some(page_text_of(&doc, index as u32 + 1)))
}

/// Extract one page (1-based, the extractor's numbering) and normalize it:
/// lines trimmed, blank lines dropped, joined with single newlines.
pub(super) fn page_text_of(doc: &Document, page_num: u32) -> String {
    let extracted = catch_unwind(AssertUnwindSafe(|| {
        let mut raw = String::new();
        {
            let mut out = PlainTextOutput::new(&mut raw);
            if output_doc_page(doc, &mut out, page_num).is_err() {
                return String::new();
            }
        }
        raw
    }))
    .unwrap_or_else(|_| {
        tracing::debug!(page = page_num, "pdf page text extraction panicked");
        String::new()
    });
    normalize_page_text(&extracted)
}

fn normalize_page_text(raw: &str) -> String {
    raw.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whitespace-separated token count across every page, for
/// `books.word_count`. `None` for a document that yields no text at all
/// (scanned pages, or one past the size cap) — unknown, not zero.
pub(super) fn word_count(path: &Path) -> Option<i64> {
    let pages = page_texts(path).ok()?;
    let words: usize = pages.iter().map(|p| p.split_whitespace().count()).sum();
    (words > 0).then_some(words as i64)
}
