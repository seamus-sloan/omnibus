//! Per-chapter text extraction for the content FTS index: walk an EPUB's
//! spine, skip navigation documents, strip markup, and emit one collapsed
//! plain-text string per spine position.
//!
// TODO: unify with the chapter-text helper being extracted from
// db/src/ebook/wordcount.rs (OMNI-2281). This walk deliberately duplicates
// that module's navigation-skip and tag-strip behaviour rather than touching
// the file mid-refactor; once the shared helper lands, this module should
// consume it and drop the copies below.

use std::path::Path;

use epub::doc::EpubDoc;

/// `epub:type` declarations marking a document as navigation rather than
/// reading, in both quote styles. Mirrors `NAV_EPUB_TYPE_ATTRS` in
/// `db/src/ebook/wordcount.rs`: an EPUB 2 HTML table of contents is an
/// ordinary spine item, so its own declaration is the only signal. Indexing
/// it would answer every chapter-title query with the ToC instead of the
/// chapter.
const NAV_EPUB_TYPE_ATTRS: [&str; 6] = [
    "epub:type=\"toc\"",
    "epub:type='toc'",
    "epub:type=\"landmarks\"",
    "epub:type='landmarks'",
    "epub:type=\"page-list\"",
    "epub:type='page-list'",
];

/// One spine item's extracted text, ready to insert into
/// `book_content_chapters`.
pub struct ChapterText {
    /// Zero-based spine position — the citation key a search hit reports.
    pub spine_index: i64,
    /// Markup-stripped, whitespace-collapsed chapter prose.
    pub text: String,
}

/// Extract every readable chapter's plain text from the EPUB at `path`.
///
/// `None` when the file won't open as an EPUB or no spine resource loads —
/// "couldn't read", which the backfill logs and retries next scan. `Some`
/// with fewer entries than spine positions is normal: navigation documents
/// and empty chapters are dropped, keeping the surviving entries' original
/// spine indices so a hit still cites the true position.
pub fn extract_chapter_texts(path: &Path) -> Option<Vec<ChapterText>> {
    let mut doc = EpubDoc::new(path).ok()?;
    let nav_id = doc.get_nav_id();
    let ids: Vec<(String, bool)> = doc
        .spine
        .iter()
        .map(|item| {
            let is_nav = nav_id.as_deref() == Some(item.idref.as_str())
                || declares_property(item.properties.as_deref(), "nav");
            (item.idref.clone(), is_nav)
        })
        .collect();
    let mut out = Vec::new();
    let mut loaded_any = false;
    for (spine_index, (id, is_nav)) in ids.into_iter().enumerate() {
        let Some((html, _mime)) = doc.get_resource_str(&id) else {
            continue;
        };
        loaded_any = true;
        if is_nav || is_navigation_document(&html) {
            continue;
        }
        let text = collapse_whitespace(&strip_tags(&html));
        if text.is_empty() {
            continue;
        }
        out.push(ChapterText {
            spine_index: spine_index as i64,
            text,
        });
    }
    loaded_any.then_some(out)
}

/// Whether a space-separated `properties` attribute carries `wanted`.
fn declares_property(properties: Option<&str>, wanted: &str) -> bool {
    properties.is_some_and(|ps| ps.split_ascii_whitespace().any(|p| p == wanted))
}

/// Whether a content document declares itself navigation via `epub:type`.
/// A substring test, not a parse: no chapter of prose carries
/// `epub:type="toc"` by accident.
fn is_navigation_document(html: &str) -> bool {
    NAV_EPUB_TYPE_ATTRS.iter().any(|attr| html.contains(attr))
}

/// Join whitespace-separated tokens with single spaces, so the stored text
/// (and the snippets `snippet()` cuts from it) reads as one line of prose
/// rather than the source document's indentation.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Minimal HTML tag stripper for FTS input — good enough for an index, not a
/// renderer: drops tags, comments, and the bodies of `script`/`style`
/// elements (indexing a stylesheet's selectors would answer prose queries
/// with CSS). Each stripped tag becomes a space so words in adjacent
/// elements don't fuse into one token.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    // A comment runs to `-->`, not to the first `>` inside it; track the
    // last two chars so `<!-- <style> -->` can't open an unclosed
    // suppression.
    let mut in_comment = false;
    let mut prev2 = '\0';
    let mut prev1 = '\0';
    let mut suppressed: Option<&'static str> = None;
    for ch in html.chars() {
        if in_comment {
            if ch == '>' && prev1 == '-' && prev2 == '-' {
                in_comment = false;
                in_tag = false;
                tag.clear();
            }
            prev2 = prev1;
            prev1 = ch;
            continue;
        }
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                in_tag = false;
                out.push(' ');
                suppressed = match suppressed {
                    Some(name) => (!closes(&tag, name)).then_some(name),
                    // A self-closing `<script/>` has no body to suppress.
                    None if !tag.ends_with('/') => suppressible(&tag),
                    None => None,
                };
            }
            _ if in_tag => {
                tag.push(ch);
                if tag == "!--" {
                    in_comment = true;
                    prev2 = '-';
                    prev1 = '-';
                }
            }
            _ if suppressed.is_some() => {}
            _ => out.push(ch),
        }
    }
    out
}

/// The suppressible element a start tag opens, if any.
fn suppressible(tag: &str) -> Option<&'static str> {
    let name = tag_name(tag)?;
    ["script", "style"]
        .into_iter()
        .find(|e| name.eq_ignore_ascii_case(e))
}

/// Whether a tag closes the named element.
fn closes(tag: &str, name: &str) -> bool {
    tag.strip_prefix('/')
        .and_then(tag_name)
        .is_some_and(|n| n.eq_ignore_ascii_case(name))
}

/// The element name at the head of a tag's contents (`p class="x"` → `p`).
fn tag_name(tag: &str) -> Option<&str> {
    let name = tag
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .filter(|n| !n.is_empty())?;
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        .then_some(name)
}
