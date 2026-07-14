//! Server-side markdown rendering for journal entries.
//!
//! User-authored markdown is rendered to HTML and then sanitized with a strict
//! `ammonia` allowlist — never trust the raw output. A custom `||spoiler||`
//! pass runs over the source first, wrapping spoiler regions in a native
//! `<button class="spoiler" type="button" aria-expanded="false">` — a real
//! button is focusable and Enter/Space-actionable for free, so keyboard and
//! screen-reader users get the same reveal affordance as mouse users. The
//! book-detail page blurs it until clicked.

use pulldown_cmark::{html, Options, Parser};

/// Path prefix every embedded journal image must be served from. The
/// sanitizer drops any `<img>` whose `src` points elsewhere, so entries can't
/// embed cross-origin trackers or arbitrary remote content.
pub const IMAGE_URL_PREFIX: &str = "/api/journals/images/";

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
    wrap_figures(&sanitize(&raw_html))
}

/// Sanitize rendered HTML, additionally permitting the spoiler `<button>` the
/// spoiler pass introduces and the read-only task-list checkbox pulldown-cmark
/// emits for `- [ ]` items.
fn sanitize(html: &str) -> String {
    let cleaned = ammonia::Builder::default()
        .add_tags(["input", "button"])
        // Embedded journal images: relative `src` values must survive (the
        // default policy strips them), but only ones under our own serving
        // prefix — anything else (absolute URLs included) loses its `src` and
        // the bare `<img>` is dropped wholesale by `drop_srcless_imgs`.
        .url_relative(ammonia::UrlRelative::PassThrough)
        .attribute_filter(|element, attribute, value| {
            if element == "img" && attribute == "src" && !value.starts_with(IMAGE_URL_PREFIX) {
                return None;
            }
            Some(value.into())
        })
        // Spoiler wrapper: only the `spoiler` class, `type="button"` (pinned via
        // value allowlist so an arbitrary type like `submit` can't leak in),
        // and `aria-expanded` (kept in sync by the client-side toggle handler).
        .add_allowed_classes("button", ["spoiler"])
        .add_tag_attribute_values("button", "type", ["button"])
        .add_tag_attribute_values("button", "aria-expanded", ["false", "true"])
        // Task-list checkboxes only — `disabled`/`checked` are valueless flags;
        // `type` is pinned to `checkbox` via the value allowlist (kept out of
        // the generic attribute set, which would otherwise permit any value).
        .add_tag_attributes("input", ["checked", "disabled"])
        .add_tag_attribute_values("input", "type", ["checkbox"])
        .add_allowed_classes("input", ["task-list-item-checkbox"])
        .clean(html)
        .to_string();
    drop_srcless_imgs(&drop_non_checkbox_inputs(&cleaned))
}

/// Remove every `<img>` without a `src` attribute. The `attribute_filter` in
/// [`sanitize`] strips off-prefix `src` values but ammonia keeps the (now
/// useless) tag, which would render as a broken-image placeholder. Runs on
/// already-sanitized HTML, so scanning to the next `>` bounds each tag (no
/// allowlisted attribute can contain one).
fn drop_srcless_imgs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<img") {
        out.push_str(&rest[..start]);
        let tag_rest = &rest[start..];
        let Some(end) = tag_rest.find('>').map(|i| i + 1) else {
            // No closing `>` — malformed; drop the remainder rather than emit it.
            return out;
        };
        let tag = &tag_rest[..end];
        if tag.contains(" src=\"") {
            out.push_str(tag);
        }
        rest = &tag_rest[end..];
    }
    out.push_str(rest);
    out
}

/// Upgrade a paragraph that contains exactly one image (and nothing else)
/// into `<figure><img …><figcaption>alt</figcaption></figure>` so embedded
/// images render as captioned figures — the alt text doubles as the caption.
/// Inline images (an `<img>` amid other paragraph content) are left as-is.
/// Runs after [`sanitize`], so the only `<p><img…></p>` shapes seen here are
/// ones the sanitizer produced.
fn wrap_figures(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<p><img ") {
        let after_p = &rest[start + 3..]; // keep `<img …` at the head
        let Some(img_end) = after_p.find('>').map(|i| i + 1) else {
            break;
        };
        // Only a lone image directly followed by the paragraph close counts.
        if !after_p[img_end..].starts_with("</p>") {
            out.push_str(&rest[..start + 3]);
            rest = after_p;
            continue;
        }
        let img = &after_p[..img_end];
        out.push_str(&rest[..start]);
        out.push_str("<figure class=\"journal-figure\">");
        out.push_str(img);
        if let Some(alt) = attr_value(img, "alt").filter(|a| !a.trim().is_empty()) {
            // The attribute value is already HTML-escaped, which is equally
            // valid as element text — copy it through verbatim.
            out.push_str("<figcaption>");
            out.push_str(alt);
            out.push_str("</figcaption>");
        }
        out.push_str("</figure>");
        rest = &after_p[img_end + 4..];
    }
    out.push_str(rest);
    out
}

/// The raw (still-escaped) value of `name="…"` in `tag`, if present.
fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
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
/// `<button class="spoiler" type="button" aria-expanded="false">text</button>`
/// HTML, which pulldown-cmark passes through (and whose inner text still gets
/// markdown inline processing). Emitting a native button — rather than a
/// `<span>` with click-only handling — means Tab reaches it, Enter/Space
/// activate it, and screen readers announce it as a button. Pairs are matched
/// greedily left-to-right; an unterminated trailing `||` is left literal.
fn wrap_spoilers(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(open) = rest.find("||") {
        let after = &rest[open + 2..];
        match after.find("||") {
            Some(close) => {
                out.push_str(&rest[..open]);
                out.push_str("<button class=\"spoiler\" type=\"button\" aria-expanded=\"false\">");
                out.push_str(&after[..close]);
                out.push_str("</button>");
                rest = &after[close + 2..];
            }
            None => break, // no closing marker — emit the remainder verbatim
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
