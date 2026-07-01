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
    let cleaned = ammonia::Builder::default()
        .add_tags(["span", "input"])
        .add_allowed_classes("span", ["spoiler"])
        // Task-list checkboxes only — `disabled`/`checked` are valueless flags;
        // `type` is pinned to `checkbox` via the value allowlist (kept out of
        // the generic attribute set, which would otherwise permit any value).
        .add_tag_attributes("input", ["checked", "disabled"])
        .add_tag_attribute_values("input", "type", ["checkbox"])
        .add_allowed_classes("input", ["task-list-item-checkbox"])
        .clean(html)
        .to_string();
    drop_non_checkbox_inputs(&cleaned)
}

/// Remove every `<input>` that isn't a disabled task-list checkbox.
///
/// `ammonia` strips *disallowed attributes* but keeps an *allowed tag*, so a
/// hand-authored `<input type="text">` degrades to a bare `<input>` — which the
/// browser renders as a text field — and would otherwise survive. We only ever
/// emit `<input>` for task-list checkboxes (`<input disabled type="checkbox">`),
/// so any tag missing either marker is user-authored and dropped wholesale.
/// Runs on already-sanitized HTML, so the only attributes an `<input>` can
/// carry are the allowlisted `checked`/`disabled`/`type="checkbox"`/`class`,
/// none of which contain `>`; scanning to the next `>` therefore bounds the tag.
/// Attribute presence is matched on real name boundaries (not raw substrings),
/// so a lookalike like `aria-disabled` can't sneak an interactive input through.
fn drop_non_checkbox_inputs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<input") {
        out.push_str(&rest[..start]);
        let tag_rest = &rest[start..];
        let Some(end) = tag_rest.find('>').map(|i| i + 1) else {
            // No closing `>` — malformed; drop the remainder rather than emit it.
            return out;
        };
        let tag = &tag_rest[..end];
        if has_attr_value(tag, "type", "checkbox") && has_bool_attr(tag, "disabled") {
            out.push_str(tag);
        }
        rest = &tag_rest[end..];
    }
    out.push_str(rest);
    out
}

/// Whether `tag` carries a standalone boolean attribute `name` — matched on
/// name boundaries so `aria-disabled` / `notdisabled` don't count, and tolerant
/// of both the bare form and a serializer's `name=""` (ammonia emits the latter).
fn has_bool_attr(tag: &str, name: &str) -> bool {
    tag.match_indices(name).any(|(i, _)| {
        let preceded_by_space = tag[..i]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
        let boundary = tag[i + name.len()..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || matches!(c, '=' | '>' | '/'));
        preceded_by_space && boundary
    })
}

/// Whether `tag` carries `name="value"` as a real attribute (name preceded by
/// whitespace), not a substring of a longer name like `data-type="checkbox"`.
fn has_attr_value(tag: &str, name: &str, value: &str) -> bool {
    let needle = format!("{name}=\"{value}\"");
    tag.match_indices(needle.as_str()).any(|(i, _)| {
        tag[..i]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    })
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
    fn strips_non_checkbox_input_element_entirely() {
        // ammonia would keep the allowlisted `input` tag while stripping its
        // disallowed attributes, leaving a bare `<input>` (a text field by
        // default). The whole element — not just its attributes — must go.
        let html = render("<input type=\"text\" value=\"x\">");
        assert!(!html.contains("<input"), "input element removed: {html}");
        assert!(!html.contains("type=\"text\""), "type dropped: {html}");
        assert!(!html.contains("value="), "value dropped: {html}");
    }

    #[test]
    fn strips_button_and_bare_inputs_entirely() {
        // Non-checkbox inputs of any flavour, including an attribute-less one and
        // an *interactive* (non-disabled) checkbox, must not survive — each
        // degrades to a bare/enabled `<input>` under ammonia. Only the disabled
        // task-list checkbox is allowed through.
        for body in [
            "<input type=\"button\">",
            "<input>",
            "<input type=\"checkbox\">",
        ] {
            let html = render(body);
            assert!(!html.contains("<input"), "no input from {body:?}: {html}");
        }
    }

    #[test]
    fn drop_non_checkbox_inputs_matches_real_attribute_tokens() {
        // A genuine disabled task-list checkbox survives.
        assert!(
            drop_non_checkbox_inputs("<input disabled=\"\" type=\"checkbox\">").contains("<input"),
            "real task-list checkbox kept"
        );
        // Lookalike attribute names must not satisfy the `disabled` check, so an
        // interactive checkbox cannot slip through on a substring match.
        for tag in [
            "<input aria-disabled=\"true\" type=\"checkbox\">",
            "<input data-disabled=\"\" type=\"checkbox\">",
            "<input notdisabled=\"\" type=\"checkbox\">",
        ] {
            assert!(
                !drop_non_checkbox_inputs(tag).contains("<input"),
                "lookalike disabled attr rejected: {tag}"
            );
        }
    }

    #[test]
    fn disallows_arbitrary_span_classes() {
        // Only the `spoiler` class survives; a hand-authored class is dropped.
        let html = render("<span class=\"evil\">x</span>");
        assert!(!html.contains("evil"), "non-spoiler class dropped: {html}");
    }
}
