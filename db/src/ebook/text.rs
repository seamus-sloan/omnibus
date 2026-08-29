//! Plain-text extraction of one EPUB spine document, for the chapter-text
//! REST read: opens the archive, strips markup via the shared
//! `ebook::strip` walk, and normalizes whitespace into line and paragraph
//! breaks. Called by `server::backend::ebooks::chapters`.

use std::path::Path;

use anyhow::Context;
use epub::doc::EpubDoc;

#[cfg(test)]
mod tests;

/// Elements whose text never belongs in served prose: the wordcount pair
/// plus `title`, whose `<head>` text would otherwise lead every chapter.
const SUPPRESSED_ELEMENTS: [&str; 3] = ["script", "style", "title"];

/// Extract the plain text of the spine document at `spine_index`.
///
/// `Ok(None)` when the spine has no such index — the caller's not-found —
/// and `Err` for the open/parse/read failure space (a foreign archive can
/// fail arbitrarily, so the message carries the path and index rather than
/// enumerating variants).
pub fn extract_chapter_text(path: &Path, spine_index: usize) -> anyhow::Result<Option<String>> {
    let mut doc = EpubDoc::new(path).with_context(|| format!("open epub {}", path.display()))?;
    let Some(item) = doc.spine.get(spine_index) else {
        return Ok(None);
    };
    let idref = item.idref.clone();
    let (html, _mime) = doc.get_resource_str(&idref).ok_or_else(|| {
        anyhow::anyhow!(
            "spine item {spine_index} ({idref}) unreadable in {}",
            path.display()
        )
    })?;
    Ok(Some(normalize_text(&super::strip::strip_markup(
        &html,
        &SUPPRESSED_ELEMENTS,
        true,
    ))))
}

/// Collapse the stripped walk's whitespace into readable plain text: a run
/// holding two or more newlines becomes a paragraph break (`"\n\n"`), a run
/// with exactly one keeps a single line break, any other run a space; both
/// ends are trimmed.
fn normalize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut newlines_in_run = 0usize;
    let mut in_run = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            in_run = true;
            if ch == '\n' {
                newlines_in_run += 1;
            }
            continue;
        }
        if in_run && !out.is_empty() {
            match newlines_in_run {
                0 => out.push(' '),
                1 => out.push('\n'),
                _ => out.push_str("\n\n"),
            }
        }
        in_run = false;
        newlines_in_run = 0;
        out.push(ch);
    }
    out
}
