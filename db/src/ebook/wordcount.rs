//! Rough word-count estimate for an already-parsed EPUB. `db::stats::pages`
//! reads the stored figure as the weakest rung of its length ladder — used only
//! when a book has neither a print-edition page count nor a comic's exact image
//! count — and converts it to pages at 275 words each.

use epub::doc::EpubDoc;

/// Elements whose text content is markup, not prose. [`strip_tags`] drops what
/// is *between* their tags as well as the tags themselves; counting a
/// stylesheet's selectors or a script's identifiers as words inflated the
/// estimate of every book that inlines either.
const SUPPRESSED_ELEMENTS: [&str; 2] = ["script", "style"];

/// `epub:type` declarations that mark a document as navigation rather than
/// reading, written out as the literal attribute text in both quote styles.
/// The manifest's `properties="nav"` covers EPUB 3 (see `EpubDoc::get_nav_id`),
/// but an EPUB 2 HTML table of contents is an ordinary spine item with nothing
/// in the manifest to distinguish it, so its own declaration is the only signal
/// available.
///
/// Spelled out rather than built with `format!` per spine item: this runs for
/// every document of every book on an index or a library-wide backfill, and the
/// patterns are fixed.
const NAV_EPUB_TYPE_ATTRS: [&str; 6] = [
    "epub:type=\"toc\"",
    "epub:type='toc'",
    "epub:type=\"landmarks\"",
    "epub:type='landmarks'",
    "epub:type=\"page-list\"",
    "epub:type='page-list'",
];

/// Estimate a book's total word count by walking every spine item's HTML
/// resource, stripping markup, and counting whitespace-separated tokens.
/// `None` when the epub has no spine or every resource fails to load — an
/// honest "couldn't estimate" rather than a fabricated zero, so the stats
/// aggregation can tell "no data" apart from "a zero-length book".
///
/// Navigation documents are skipped. A table of contents is a list of the
/// book's own chapter titles, so counting it charges a long book twice for
/// every heading it has — an error that grows with the book, which is the worst
/// shape an estimate's error can take.
pub fn estimate_word_count<R: std::io::Read + std::io::Seek>(doc: &mut EpubDoc<R>) -> Option<i64> {
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
    if ids.is_empty() {
        return None;
    }
    let mut total: i64 = 0;
    let mut loaded_any = false;
    for (id, is_nav) in ids {
        let Some((html, _mime)) = doc.get_resource_str(&id) else {
            continue;
        };
        // Counted as loaded even when skipped: a book whose spine is nothing
        // but navigation was read successfully and estimates at zero, which is
        // a different fact from one whose resources wouldn't open at all.
        loaded_any = true;
        if is_nav || is_navigation_document(&html) {
            continue;
        }
        total += strip_tags(&html).split_whitespace().count() as i64;
    }
    loaded_any.then_some(total)
}

/// Whether a space-separated `properties` attribute carries `wanted`.
///
/// Belt-and-braces for a malformed package: `nav` is a *manifest* item
/// property, which `EpubDoc::get_nav_id` already reads, and the attribute
/// reached here is the spine `itemref`'s (`page-spread-*`, `rendition:*`). A
/// well-formed EPUB will never declare `nav` on it.
fn declares_property(properties: Option<&str>, wanted: &str) -> bool {
    properties.is_some_and(|ps| ps.split_ascii_whitespace().any(|p| p == wanted))
}

/// Whether a content document declares itself navigation via `epub:type`.
/// A substring test rather than a parse: this runs once per spine item during
/// an index, and no chapter of prose carries `epub:type="toc"` by accident.
fn is_navigation_document(html: &str) -> bool {
    NAV_EPUB_TYPE_ATTRS.iter().any(|attr| html.contains(attr))
}

/// Minimal HTML tag stripper — good enough for a word-count estimate, not a
/// renderer: drops everything between (and including) `<` / `>`, plus the
/// bodies of the [`SUPPRESSED_ELEMENTS`] and the contents of comments.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    // A comment runs to `-->`, not to the first `>` inside it. Without this
    // state a commented-out `<style>` opens a suppression that nothing closes
    // — the `>` of `-->` isn't a close tag — and the rest of the document is
    // silently dropped from the count.
    //
    // Only the two characters before a `>` decide whether it terminates, so the
    // comment body is tracked as a rolling pair rather than buffered: EPUB
    // XHTML carries licence blocks and generator preambles that would otherwise
    // be copied in full for nothing.
    let mut in_comment = false;
    let mut prev2 = '\0';
    let mut prev1 = '\0';
    // The element whose text is currently being discarded, or `None` while text
    // is being kept. Not a depth counter: neither `script` nor `style` nests.
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
                suppressed = match suppressed {
                    Some(name) => (!closes(&tag, name)).then_some(name),
                    // A self-closing `<script/>` has no body to suppress, and
                    // treating it as an opener would swallow the rest of the
                    // document.
                    None if !tag.ends_with('/') => suppressible(&tag),
                    None => None,
                };
            }
            _ if in_tag => {
                tag.push(ch);
                if tag == "!--" {
                    // Seed the pair with the dashes just consumed, so the empty
                    // comment `<!-->` still terminates on its own `>`.
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

/// The [`SUPPRESSED_ELEMENTS`] entry a start tag opens, if any.
fn suppressible(tag: &str) -> Option<&'static str> {
    let name = tag_name(tag)?;
    SUPPRESSED_ELEMENTS
        .iter()
        .copied()
        .find(|e| name.eq_ignore_ascii_case(e))
}

/// Whether a tag closes the named element.
fn closes(tag: &str, name: &str) -> bool {
    tag.strip_prefix('/')
        .and_then(tag_name)
        .is_some_and(|n| n.eq_ignore_ascii_case(name))
}

/// The element name at the head of a tag's contents (`p class="x"` → `p`).
/// `None` for a comment, a processing instruction, or an empty tag — anything
/// not starting with a letter isn't an element name.
fn tag_name(tag: &str) -> Option<&str> {
    let name = tag
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .filter(|n| !n.is_empty())?;
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        .then_some(name)
}

#[cfg(test)]
mod tests;
