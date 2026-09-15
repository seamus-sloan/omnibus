//! A PDF's structure in the EPUB structure tables' vocabulary: one spine
//! entry per page, weighted by its extracted text, and the outline as the
//! chapter list. Written through `epub_structure::replace_structure` so the
//! anchor index, the progress enrichment, and the spoiler boundary read PDFs
//! exactly as they read EPUBs.

use std::path::Path;

use pdf_extract::Document;

use super::text;
use crate::ebook::toc::{EpubStructure, SpineStat, TocChapter};

/// Extract the per-page spine stats and the outline. `Ok(None)` for a
/// document with no pages; `Err` when the file is unreadable or past the
/// text size cap. A page with no text still weighs one unit, so a scanned
/// book tiles the percent ruler one page per step instead of collapsing to
/// a single point.
pub fn extract_structure(path: &Path) -> anyhow::Result<Option<EpubStructure>> {
    let doc = text::load_document(path)?;
    let count = doc.get_pages().len();
    if count == 0 {
        return Ok(None);
    }
    let mut spine = Vec::with_capacity(count);
    let mut cumulative = Vec::with_capacity(count);
    let mut acc: i64 = 0;
    for page in 0..count {
        let chars = text::page_text_of(&doc, page as u32 + 1).chars().count() as i64;
        let visible_chars = chars.max(1);
        cumulative.push(acc);
        acc += visible_chars;
        spine.push(SpineStat {
            spine_index: page as i64,
            href: page_href(page),
            visible_chars,
        });
    }
    let chapters = outline_chapters(&doc, &cumulative);
    Ok(Some(EpubStructure { spine, chapters }))
}

/// The pseudo-href a page spine entry carries — `page:N` — so the stored row
/// says what it is rather than pretending to be a document path.
pub fn page_href(page: usize) -> String {
    format!("page:{page}")
}

/// The outline flattened in document order onto the pages it points at.
/// Entries pointing outside the page tree or carrying a blank title are
/// dropped rather than guessed at; an outline the extractor can't read is an
/// empty list, the same honest "no TOC" an EPUB without one reports.
fn outline_chapters(doc: &Document, cumulative: &[i64]) -> Vec<TocChapter> {
    let Ok(toc) = doc.get_toc() else {
        return Vec::new();
    };
    toc.toc
        .iter()
        .filter_map(|entry| {
            let title = entry.title.trim();
            let page = entry.page.checked_sub(1)?;
            let start_chars = *cumulative.get(page)?;
            (!title.is_empty()).then(|| (title.to_string(), page, start_chars))
        })
        .enumerate()
        .map(|(ordinal, (title, page, start_chars))| TocChapter {
            ordinal: ordinal as i64,
            title,
            href: page_href(page),
            spine_index: page as i64,
            start_chars,
        })
        .collect()
}
