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
    // Task lists (`- [ ]` / `- [x]`) back the editor's checklist button. They
    // render as a disabled `<input type="checkbox">` which the sanitizer keeps
    // in a tightly constrained form (see `sanitize`).
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&with_spoilers, opts);
    let mut raw_html = String::new();
    html::push_html(&mut raw_html, parser);
    sanitize(&raw_html)
}

/// Sanitize rendered HTML, additionally permitting `<span class="spoiler">`
/// (the only inline element the spoiler pass introduces) and the read-only
/// task-list checkbox pulldown-cmark emits for `- [ ]` items.
fn sanitize(html: &str) -> String {
    ammonia::Builder::default()
        .add_tags(["span", "input"])
        .add_allowed_classes("span", ["spoiler"])
        // Task-list checkboxes only — `disabled`/`checked` are valueless flags;
        // `type` is pinned to `checkbox` via the value allowlist (kept out of
        // the generic attribute set, which would otherwise permit any value),
        // so no interactive or arbitrary `<input>` survives.
        .add_tag_attributes("input", ["checked", "disabled"])
        .add_tag_attribute_values("input", "type", ["checkbox"])
        .add_allowed_classes("input", ["task-list-item-checkbox"])
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
    fn renders_task_list_checkboxes() {
        let html = render("- [x] done\n- [ ] todo");
        // A checked + an unchecked disabled checkbox survive sanitization.
        assert!(html.contains("type=\"checkbox\""), "got: {html}");
        assert!(html.contains("checked"), "checked box kept: {html}");
        assert!(html.contains("disabled"), "boxes stay read-only: {html}");
    }

    #[test]
    fn task_list_input_type_is_pinned_to_checkbox() {
        // A hand-authored non-checkbox input must not survive even though the
        // `input` tag is now allowlisted for task lists.
        let html = render("<input type=\"text\" value=\"x\">");
        assert!(
            !html.contains("type=\"text\""),
            "non-checkbox dropped: {html}"
        );
        assert!(!html.contains("value="), "input value dropped: {html}");
    }

    #[test]
    fn disallows_arbitrary_span_classes() {
        // Only the `spoiler` class survives; a hand-authored class is dropped.
        let html = render("<span class=\"evil\">x</span>");
        assert!(!html.contains("evil"), "non-spoiler class dropped: {html}");
    }
}
