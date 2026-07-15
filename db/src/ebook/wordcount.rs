//! Rough word-count estimate for an already-parsed EPUB. `db::stats::pages`
//! calls this on demand for books finished within a stats window and
//! converts the total to an estimated page count for the stats page's Pages
//! tile — see that module's doc comment for the full sourcing rationale.

use epub::doc::EpubDoc;

/// Estimate a book's total word count by walking every spine item's HTML
/// resource, stripping markup, and counting whitespace-separated tokens.
/// `None` when the epub has no spine or every resource fails to load — an
/// honest "couldn't estimate" rather than a fabricated zero, so the stats
/// aggregation can tell "no data" apart from "a zero-length book".
pub fn estimate_word_count<R: std::io::Read + std::io::Seek>(doc: &mut EpubDoc<R>) -> Option<i64> {
    let ids: Vec<String> = doc.spine.iter().map(|item| item.idref.clone()).collect();
    if ids.is_empty() {
        return None;
    }
    let mut total: i64 = 0;
    let mut loaded_any = false;
    for id in ids {
        if let Some((html, _mime)) = doc.get_resource_str(&id) {
            total += strip_tags(&html).split_whitespace().count() as i64;
            loaded_any = true;
        }
    }
    loaded_any.then_some(total)
}

/// Minimal HTML tag stripper — good enough for a word-count estimate, not a
/// renderer: drops everything between (and including) `<` / `>`, keeps the
/// rest. EPUB XHTML content documents essentially never carry `<script>` /
/// `<style>` bodies, so no special-casing for those is needed here.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
