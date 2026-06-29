//! Server-side markdown rendering for journal entries.
//!
//! User-authored markdown is rendered to HTML and then sanitized with a strict
//! `ammonia` allowlist — never trust the raw output. A custom `||spoiler||`
//! pass runs over the source first, wrapping spoiler regions in a
//! `<span class="spoiler">` that the sanitizer is configured to keep (and the
//! book-detail page blurs until clicked).

use pulldown_cmark::{html, Options, Parser};

/// Render a journal body's markdown to sanitized HTML safe for
/// `dangerous_inner_html`.
pub fn render(md: &str) -> String {
    let with_spoilers = wrap_spoilers(md);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(&with_spoilers, opts);
    let mut raw_html = String::new();
    html::push_html(&mut raw_html, parser);
    sanitize(&raw_html)
}

/// Sanitize rendered HTML, additionally permitting `<span class="spoiler">`
/// (the only inline element the spoiler pass introduces).
fn sanitize(html: &str) -> String {
    ammonia::Builder::default()
        .add_tags(["span"])
        .add_allowed_classes("span", ["spoiler"])
        .clean(html)
        .to_string()
}

/// Rewrite `||text||` spoiler markers in the markdown **source** into inline
/// `<span class="spoiler">text</span>` HTML, which pulldown-cmark passes through
/// (and whose inner text still gets markdown inline processing). Pairs are
/// matched greedily left-to-right; an unterminated trailing `||` is left
/// literal.
fn wrap_spoilers(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(open) = rest.find("||") {
        let after = &rest[open + 2..];
        match after.find("||") {
            Some(close) => {
                out.push_str(&rest[..open]);
                out.push_str("<span class=\"spoiler\">");
                out.push_str(&after[..close]);
                out.push_str("</span>");
                rest = &after[close + 2..];
            }
            None => break, // no closing marker — emit the remainder verbatim
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_markdown() {
        let html = render("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"), "got: {html}");
        assert!(html.contains("<em>italic</em>"), "got: {html}");
    }

    #[test]
    fn renders_strikethrough() {
        let html = render("~~gone~~");
        assert!(html.contains("<del>gone</del>"), "got: {html}");
    }

    #[test]
    fn strips_script_tags() {
        let html = render("hi <script>alert('x')</script> there");
        assert!(!html.contains("<script"), "script must be stripped: {html}");
        assert!(!html.contains("alert("), "script body removed: {html}");
    }

    #[test]
    fn strips_event_handler_attributes() {
        let html = render("<a href=\"#\" onclick=\"steal()\">link</a>");
        assert!(!html.contains("onclick"), "handlers stripped: {html}");
    }

    #[test]
    fn wraps_spoiler_markers_in_spans() {
        let html = render("the killer is ||the butler||");
        assert!(
            html.contains("<span class=\"spoiler\">the butler</span>"),
            "got: {html}"
        );
    }

    #[test]
    fn spoiler_inner_text_is_markdown_processed() {
        let html = render("||the **butler** did it||");
        assert!(html.contains("<span class=\"spoiler\">"), "got: {html}");
        assert!(html.contains("<strong>butler</strong>"), "got: {html}");
    }

    #[test]
    fn unterminated_spoiler_marker_is_left_literal() {
        let html = render("a lone ||marker here");
        assert!(!html.contains("class=\"spoiler\""), "got: {html}");
        assert!(html.contains("||marker here"), "got: {html}");
    }

    #[test]
    fn disallows_arbitrary_span_classes() {
        // Only the `spoiler` class survives; a hand-authored class is dropped.
        let html = render("<span class=\"evil\">x</span>");
        assert!(!html.contains("evil"), "non-spoiler class dropped: {html}");
    }
}
