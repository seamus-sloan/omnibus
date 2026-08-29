//! Shared HTML markup-stripping walk over an EPUB content document.
//! `wordcount` counts the emitted text and `text` serves it as prose, so
//! both call this one implementation of tag/comment/suppression handling
//! rather than maintaining two strippers that drift apart.

/// Elements that imply a line boundary in extracted prose. When
/// `block_breaks` is on, [`strip_markup`] emits a `'\n'` for each of these
/// tags (open, close, or self-closing) so `</p><p>` doesn't run two
/// paragraphs together — while inline tags (`<i>`, `<span>`) still emit
/// nothing, keeping a word split across them whole.
const BLOCK_ELEMENTS: [&str; 33] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "br",
    "caption",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "li",
    "main",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "td",
    "tr",
    "ul",
];

/// Minimal HTML tag stripper — good enough for derived text, not a
/// renderer: drops everything between (and including) `<` / `>`, plus the
/// bodies of the `suppressed_elements` and the contents of comments.
/// `block_breaks` additionally emits a newline per [`BLOCK_ELEMENTS`] tag.
pub(crate) fn strip_markup(html: &str, suppressed_elements: &[&str], block_breaks: bool) -> String {
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    // A comment runs to `-->`, not to the first `>` inside it. Without this
    // state a commented-out `<style>` opens a suppression that nothing closes
    // — the `>` of `-->` isn't a close tag — and the rest of the document is
    // silently dropped.
    //
    // Only the two characters before a `>` decide whether it terminates, so the
    // comment body is tracked as a rolling pair rather than buffered: EPUB
    // XHTML carries licence blocks and generator preambles that would otherwise
    // be copied in full for nothing.
    let mut in_comment = false;
    let mut prev2 = '\0';
    let mut prev1 = '\0';
    // The element whose text is currently being discarded, or `None` while text
    // is being kept. Not a depth counter: none of the suppressed elements nest.
    let mut suppressed: Option<&str> = None;
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
                    None if !tag.ends_with('/') => suppressible(&tag, suppressed_elements),
                    None => None,
                };
                if block_breaks && suppressed.is_none() && is_block_tag(&tag) {
                    out.push('\n');
                }
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

/// The `suppressed_elements` entry a start tag opens, if any.
fn suppressible<'a>(tag: &str, suppressed_elements: &[&'a str]) -> Option<&'a str> {
    let name = tag_name(tag)?;
    suppressed_elements
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

/// Whether a tag (open or close) names one of the [`BLOCK_ELEMENTS`].
fn is_block_tag(tag: &str) -> bool {
    let named = tag.strip_prefix('/').unwrap_or(tag);
    tag_name(named).is_some_and(|n| BLOCK_ELEMENTS.iter().any(|e| n.eq_ignore_ascii_case(e)))
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
