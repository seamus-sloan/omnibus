//! Tests for journal markdown rendering: basic formatting and strikethrough,
//! HTML sanitization (script tags, event-handler attributes, non-checkbox
//! inputs), spoiler-marker wrapping, task-list checkboxes, and image
//! handling (captioned figures, offsite-source stripping).

use super::*;

#[test]
fn render_emits_strong_and_em_for_bold_and_italic_markdown() {
    let html = render("**bold** and *italic*");
    assert!(html.contains("<strong>bold</strong>"), "got: {html}");
    assert!(html.contains("<em>italic</em>"), "got: {html}");
}

#[test]
fn render_emits_del_for_strikethrough() {
    let html = render("~~gone~~");
    assert!(html.contains("<del>gone</del>"), "got: {html}");
}

#[test]
fn render_strips_script_tags() {
    let html = render("hi <script>alert('x')</script> there");
    assert!(!html.contains("<script"), "script must be stripped: {html}");
    assert!(!html.contains("alert("), "script body removed: {html}");
}

#[test]
fn render_strips_event_handler_attributes() {
    let html = render("<a href=\"#\" onclick=\"steal()\">link</a>");
    assert!(!html.contains("onclick"), "handlers stripped: {html}");
}

#[test]
fn render_wraps_spoiler_markers_in_keyboard_reachable_buttons() {
    // The spoiler wrapper is a real button so keyboard/screen-reader users
    // can reveal it. `type="button"` keeps it out of any surrounding form
    // (composer submits happen via a distinct Publish button), and
    // `aria-expanded="false"` seeds the collapsed state the client flips.
    let html = render("the killer is ||the butler||");
    assert!(html.contains("<button "), "spoiler is a button: {html}");
    assert!(html.contains("class=\"spoiler\""), "got: {html}");
    assert!(html.contains("type=\"button\""), "got: {html}");
    assert!(html.contains("aria-expanded=\"false\""), "got: {html}");
    assert!(html.contains(">the butler</button>"), "got: {html}");
}

#[test]
fn render_processes_markdown_inside_spoiler_text() {
    let html = render("||the **butler** did it||");
    assert!(html.contains("class=\"spoiler\""), "got: {html}");
    assert!(html.contains("<strong>butler</strong>"), "got: {html}");
}

#[test]
fn render_leaves_unterminated_spoiler_marker_literal() {
    let html = render("a lone ||marker here");
    assert!(!html.contains("class=\"spoiler\""), "got: {html}");
    assert!(!html.contains("<button"), "no button emitted: {html}");
    assert!(html.contains("||marker here"), "got: {html}");
}

#[test]
fn render_emits_task_list_checkboxes() {
    let html = render("- [x] done\n- [ ] todo");
    // A checked + an unchecked disabled checkbox survive sanitization.
    assert!(html.contains("type=\"checkbox\""), "got: {html}");
    assert!(html.contains("checked"), "checked box kept: {html}");
    assert!(html.contains("disabled"), "boxes stay read-only: {html}");
}

#[test]
fn render_strips_non_checkbox_input_element_entirely() {
    // ammonia would keep the allowlisted `input` tag while stripping its
    // disallowed attributes, leaving a bare `<input>` (a text field by
    // default). The whole element — not just its attributes — must go.
    let html = render("<input type=\"text\" value=\"x\">");
    assert!(!html.contains("<input"), "input element removed: {html}");
    assert!(!html.contains("type=\"text\""), "type dropped: {html}");
    assert!(!html.contains("value="), "value dropped: {html}");
}

#[test]
fn render_strips_button_and_bare_inputs_entirely() {
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
fn render_wraps_lone_image_as_captioned_figure() {
    let html = render("![A sunset over the bay](/api/journals/images/abc.png)");
    assert!(
        html.contains("<figure class=\"journal-figure\">"),
        "got: {html}"
    );
    assert!(
        html.contains("src=\"/api/journals/images/abc.png\""),
        "got: {html}"
    );
    assert!(
        html.contains("<figcaption>A sunset over the bay</figcaption>"),
        "got: {html}"
    );
}

#[test]
fn render_omits_figcaption_for_lone_image_without_alt() {
    let html = render("![](/api/journals/images/abc.png)");
    assert!(html.contains("<figure"), "got: {html}");
    assert!(!html.contains("<figcaption>"), "got: {html}");
}

#[test]
fn render_leaves_inline_image_amid_text_unwrapped() {
    let html = render("before ![tiny](/api/journals/images/abc.png) after");
    assert!(html.contains("<img"), "got: {html}");
    assert!(
        !html.contains("<figure"),
        "inline images stay plain: {html}"
    );
}

#[test]
fn render_strips_images_with_offsite_or_off_prefix_src() {
    for md in [
        "![x](https://evil.example/track.png)",
        "![x](/covers/1.png)",
        "![x](//evil.example/t.png)",
        "<img src=\"https://evil.example/t.png\">",
    ] {
        let html = render(md);
        assert!(
            !html.contains("<img"),
            "img from {md:?} must be dropped: {html}"
        );
    }
}

#[test]
fn render_strips_journal_figure_class_from_hand_authored_figure() {
    // ammonia's defaults keep bare `<figure>`/`<figcaption>` (harmless
    // semantic markup), but the styled class only comes from our own
    // `wrap_figures` pass — a hand-authored one is stripped.
    let html = render("<figure class=\"journal-figure\"><figcaption>fake</figcaption></figure>");
    assert!(!html.contains("journal-figure"), "got: {html}");
}

#[test]
fn render_disallows_arbitrary_span_classes() {
    // Since spoilers no longer use `<span>`, the sanitizer no longer
    // allowlists any class on it: `<span>` itself remains under ammonia's
    // default tag list, but its `class` attribute (and thus a hand-authored
    // `spoiler` or `evil` class) is stripped.
    let html = render("<span class=\"evil\">x</span>");
    assert!(!html.contains("evil"), "non-spoiler class dropped: {html}");
    assert!(!html.contains("class="), "span class attr dropped: {html}");
}

#[test]
fn render_disallows_arbitrary_button_classes_and_attrs() {
    // Only the spoiler wrapper's exact shape (`class="spoiler"`,
    // `type="button"`, `aria-expanded` ∈ {true,false}) is allowlisted; any
    // other class, an off-list `type`, or a spoofed `aria-expanded` value
    // gets stripped so a hand-authored `<button>` can't impersonate site
    // chrome or fire form submits.
    let html = render(
        "<button class=\"evil primary\" type=\"submit\" aria-expanded=\"maybe\" onclick=\"x()\">boom</button>",
    );
    assert!(!html.contains("evil"), "arbitrary class dropped: {html}");
    assert!(!html.contains("primary"), "arbitrary class dropped: {html}");
    assert!(
        !html.contains("type=\"submit\""),
        "off-list type dropped: {html}"
    );
    assert!(
        !html.contains("aria-expanded=\"maybe\""),
        "off-list aria value dropped: {html}"
    );
    assert!(!html.contains("onclick"), "handler dropped: {html}");
}
