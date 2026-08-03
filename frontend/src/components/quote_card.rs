//! Shareable quote-card editor: live preview, background presets + custom
//! color, typeface/size/style and aspect-ratio choice, and PNG
//! export/share/copy drawn by the standalone `quote-card.js` canvas renderer
//! (`window.OmnibusQuoteCard`). Shell-agnostic — the reader's quote drawer and
//! the book-detail passages modal wrap it and must load [`QUOTE_CARD_JS`].

use dioxus::prelude::*;

/// The standalone canvas renderer behind the export/share/copy actions.
/// Every page that mounts [`QuoteCardPanel`] must serve this script.
pub const QUOTE_CARD_JS: Asset = asset!("/assets/quote-card.js");

/// A background/ink preset swatch.
struct Preset {
    bg: &'static str,
    ink: &'static str,
}

/// A quote-body typeface: the token sent to `quote-card.js` (which owns the
/// matching canvas stack), the chip label, and the preview's CSS stack.
struct FontChoice {
    key: &'static str,
    label: &'static str,
    css: &'static str,
}

/// A quote-body text size: the token sent to `quote-card.js` (which owns the
/// matching canvas scale) and the preview's CSS size.
struct SizeChoice {
    key: &'static str,
    label: &'static str,
    css: &'static str,
}

const RATIOS: [&str; 4] = ["1:1", "4:5", "9:16", "3:4"];

/// Selectable typefaces. Each maps to one of the app's three loaded families,
/// so the canvas export can draw the same face the preview shows.
const FONTS: [FontChoice; 3] = [
    FontChoice {
        key: "serif",
        label: "Serif",
        css: "var(--serif)",
    },
    FontChoice {
        key: "sans",
        label: "Sans",
        css: "var(--sans)",
    },
    FontChoice {
        key: "mono",
        label: "Mono",
        css: "var(--mono)",
    },
];

/// Selectable quote-body sizes. `m` is the card's historical 22px.
const SIZES: [SizeChoice; 3] = [
    SizeChoice {
        key: "s",
        label: "S",
        css: "18px",
    },
    SizeChoice {
        key: "m",
        label: "M",
        css: "22px",
    },
    SizeChoice {
        key: "l",
        label: "L",
        css: "27px",
    },
];

/// Fixed background presets (the book-accent swatch is prepended separately).
const PRESETS: [Preset; 4] = [
    Preset {
        bg: "#161412",
        ink: "#ece6dc",
    },
    Preset {
        bg: "#f0ece2",
        ink: "#2a2520",
    },
    Preset {
        bg: "rgb(34,197,94)",
        ink: "#0a2912",
    },
    Preset {
        bg: "rgb(244,63,94)",
        ink: "#2a0810",
    },
];

/// Invoke one of `window.OmnibusQuoteCard`'s actions with a JSON payload
/// (no-op on SSR and wherever the script hasn't loaded).
#[cfg(any(feature = "web", feature = "mobile"))]
fn quote_card_call(method: &str, payload: &str) {
    let lit = crate::js_interop::json_literal(payload);
    let _ = dioxus::document::eval(&format!(
        "window.OmnibusQuoteCard && window.OmnibusQuoteCard.{method}({lit});"
    ));
}

/// The preview CSS font stack for a typeface token, falling back to the
/// default serif when the token is unrecognized.
fn font_css(key: &str) -> &'static str {
    FONTS
        .iter()
        .find(|f| f.key == key)
        .map_or(FONTS[0].css, |f| f.css)
}

/// The preview CSS font size for a size token, falling back to medium when
/// the token is unrecognized.
fn size_css(key: &str) -> &'static str {
    SIZES
        .iter()
        .find(|s| s.key == key)
        .map_or(SIZES[1].css, |s| s.css)
}

/// Pick a legible ink color for a hand-picked `#rrggbb` background via
/// relative luminance (WCAG-ish). Returns a light or dark token.
fn ink_for(hex: &str) -> &'static str {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return "#f5f0e8";
    }
    let parse = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    let lin = |c: u8| {
        let x = f64::from(c) / 255.0;
        if x <= 0.039_28 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(parse(0)) + 0.7152 * lin(parse(2)) + 0.0722 * lin(parse(4));
    if l > 0.42 {
        "#221d18"
    } else {
        "#f5f0e8"
    }
}

/// Panel head: kicker + title + close button.
fn render_quote_header(on_close: EventHandler<()>) -> Element {
    rsx! {
        div { class: "rd-drawer-head",
            div {
                div { class: "rd-quote-kicker", "From your highlight" }
                h4 { class: "rd-drawer-title", "Make a quote card" }
            }
            button {
                class: "rd-x",
                r#type: "button",
                "aria-label": "Close",
                onclick: move |_| on_close.call(()),
                "\u{00d7}"
            }
        }
    }
}

/// The live quote-card preview. `card_style` carries the background, ink,
/// ratio and typeface; `body_style` the quote's own size and weight/slant.
fn render_quote_preview(
    card_style: &str,
    body_style: &str,
    quote_text: &str,
    author: &str,
    subtitle: &str,
) -> Element {
    rsx! {
        div { class: "rd-quote-card", "data-testid": "quote-preview", style: "{card_style}",
            div { class: "rd-quote-card-head", "OMNIBUS \u{00b7} QUOTE" }
            div {
                class: "rd-quote-card-body",
                "data-testid": "quote-preview-body",
                style: "{body_style}",
                "\u{201c}{quote_text}\u{201d}"
            }
            div { class: "rd-quote-card-foot",
                div { class: "rd-quote-card-author", "{author}" }
                div { class: "rd-quote-card-sub", "{subtitle}" }
            }
        }
    }
}

/// Background swatches (book accent + fixed presets + custom color picker).
fn render_background_swatches(
    accent: &str,
    bg: Signal<String>,
    ink: Signal<String>,
    custom: Signal<String>,
) -> Element {
    let mut bg = bg;
    let mut ink = ink;
    let mut custom = custom;
    rsx! {
        div { class: "rd-quote-label", "Background" }
        div { class: "rd-quote-swatches",
            button {
                class: "rd-quote-swatch",
                r#type: "button",
                style: "background:{accent};",
                "aria-label": "Book accent",
                onclick: {
                    let accent = accent.to_string();
                    move |_| {
                        bg.set(accent.clone());
                        ink.set("#f5f0e8".into());
                    }
                },
            }
            for p in PRESETS.iter() {
                button {
                    key: "{p.bg}",
                    class: "rd-quote-swatch",
                    r#type: "button",
                    style: "background:{p.bg};",
                    "aria-label": "Background {p.bg}",
                    onclick: move |_| {
                        bg.set(p.bg.to_string());
                        ink.set(p.ink.to_string());
                    },
                }
            }
            // Transparent input over a CSS rainbow: reads as a picker, not a preset.
            label { class: "rd-quote-custom", title: "Custom color",
                input {
                    r#type: "color",
                    "aria-label": "Custom background color",
                    value: "{custom}",
                    oninput: move |e| {
                        let hex = e.value();
                        custom.set(hex.clone());
                        ink.set(ink_for(&hex).to_string());
                        bg.set(hex);
                    },
                }
            }
        }
    }
}

/// Typeface chip row — each chip previews in the face it selects. Pressed-button
/// toggle group, so the selection carries on `aria-pressed` (as in `landing/toolbar.rs`).
fn render_font_controls(font: Signal<String>) -> Element {
    let mut font = font;
    let cur = font();
    rsx! {
        div { class: "rd-quote-label", "Font" }
        div { class: "rd-quote-chips", role: "group", "aria-label": "Font",
            for f in FONTS.iter() {
                button {
                    key: "{f.key}",
                    class: if cur == f.key { "rd-chip on" } else { "rd-chip" },
                    r#type: "button",
                    style: "font-family:{f.css};",
                    "aria-pressed": "{cur == f.key}",
                    onclick: move |_| font.set(f.key.to_string()),
                    "{f.label}"
                }
            }
        }
    }
}

/// Text-size chip row (S / M / L).
fn render_size_controls(size: Signal<String>) -> Element {
    let mut size = size;
    let cur = size();
    rsx! {
        div { class: "rd-quote-label", "Size" }
        div { class: "rd-quote-chips", role: "group", "aria-label": "Text size",
            for s in SIZES.iter() {
                button {
                    key: "{s.key}",
                    class: if cur == s.key { "rd-chip on" } else { "rd-chip" },
                    r#type: "button",
                    "aria-pressed": "{cur == s.key}",
                    onclick: move |_| size.set(s.key.to_string()),
                    "{s.label}"
                }
            }
        }
    }
}

/// Weight + slant toggles. Independent, so a card can be bold, italic, both,
/// or neither; `aria-pressed` carries the state for assistive tech.
fn render_style_controls(bold: Signal<bool>, italic: Signal<bool>) -> Element {
    let mut bold = bold;
    let mut italic = italic;
    let (is_bold, is_italic) = (bold(), italic());
    rsx! {
        div { class: "rd-quote-label", "Style" }
        div { class: "rd-quote-chips", role: "group", "aria-label": "Style",
            button {
                class: if is_bold { "rd-chip on" } else { "rd-chip" },
                r#type: "button",
                style: "font-weight:700;",
                "aria-pressed": "{is_bold}",
                onclick: move |_| bold.toggle(),
                "Bold"
            }
            button {
                class: if is_italic { "rd-chip on" } else { "rd-chip" },
                r#type: "button",
                style: "font-style:italic;",
                "aria-pressed": "{is_italic}",
                onclick: move |_| italic.toggle(),
                "Italic"
            }
        }
    }
}

/// Aspect-ratio chip row.
fn render_ratio_controls(ratio: Signal<String>, cur_ratio: &str) -> Element {
    let mut ratio = ratio;
    rsx! {
        div { class: "rd-quote-label", "Aspect" }
        div { class: "rd-quote-ratios", role: "group", "aria-label": "Aspect ratio",
            for r in RATIOS {
                button {
                    key: "{r}",
                    class: if cur_ratio == r { "rd-chip on" } else { "rd-chip" },
                    r#type: "button",
                    "aria-pressed": "{cur_ratio == r}",
                    onclick: move |_| ratio.set(r.to_string()),
                    "{r}"
                }
            }
        }
    }
}

/// Tray-and-arrow mark for the download action.
fn download_glyph() -> Element {
    rsx! {
        svg {
            width: "15", height: "15", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor",
            stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "M12 3v12" }
            path { d: "m7 12 5 5 5-5" }
            path { d: "M4 21h16" }
        }
    }
}

/// Stacked-sheets mark for the copy action (mirrors the selection popover's).
fn copy_glyph() -> Element {
    rsx! {
        svg {
            width: "15", height: "15", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor",
            stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": "true",
            rect { x: "9", y: "9", width: "12", height: "12", rx: "2" }
            path { d: "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" }
        }
    }
}

/// Export actions: download (desktop), copy (both), share (phone).
fn render_quote_actions(
    download: impl FnMut(Event<MouseData>) + 'static,
    copy_image: impl FnMut(Event<MouseData>) + 'static,
    share: impl FnMut(Event<MouseData>) + 'static,
) -> Element {
    rsx! {
        div { class: "rd-quote-actions",
            button {
                class: "btn primary rd-desktop-only",
                r#type: "button",
                "data-testid": "quote-download",
                onclick: download,
                {download_glyph()}
                "Download"
            }
            button {
                class: "btn",
                r#type: "button",
                "data-testid": "quote-copy-image",
                onclick: copy_image,
                {copy_glyph()}
                "Copy"
            }
            // Phone primary action: the OS share sheet takes the
            // rendered PNG (quote-card.js falls back to a download where
            // Web Share can't carry files).
            button {
                class: "btn primary rd-phone-only",
                r#type: "button",
                "data-testid": "quote-share",
                onclick: share,
                "Share"
            }
        }
    }
}

/// Quote-card editor panel: head + preview + controls. Shell-agnostic — the
/// reader wraps it in its right-hand drawer, book detail in a centered modal.
#[component]
pub fn QuoteCardPanel(
    quote_text: String,
    author: String,
    subtitle: String,
    accent: String,
    on_close: EventHandler<()>,
) -> Element {
    let bg = use_signal(|| accent.clone());
    let ink = use_signal(|| "#f5f0e8".to_string());
    let ratio = use_signal(|| "1:1".to_string());
    let custom = use_signal(|| "#6b4f8a".to_string());
    let font = use_signal(|| FONTS[0].key.to_string());
    let size = use_signal(|| SIZES[1].key.to_string());
    let bold = use_signal(|| false);
    // The card's designed default is an italic serif pull-quote.
    let italic = use_signal(|| true);

    let cur_bg = bg();
    let cur_ink = ink();
    let cur_ratio = ratio();
    let card_style = format!(
        "aspect-ratio:{}; background:{cur_bg}; color:{cur_ink}; font-family:{};",
        cur_ratio.replace(':', "/"),
        font_css(&font())
    );
    let body_style = format!(
        "font-size:{}; font-weight:{}; font-style:{};",
        size_css(&size()),
        if bold() { "700" } else { "400" },
        if italic() { "italic" } else { "normal" },
    );

    // One canvas payload for all three actions (download / share / copy);
    // `Clone` so each button handler owns a copy. Only the interactive
    // targets build it — `serde_json` isn't compiled in on SSR.
    #[cfg(any(feature = "web", feature = "mobile"))]
    let build_payload = {
        let text = quote_text.clone();
        let author = author.clone();
        let subtitle = subtitle.clone();
        move || {
            serde_json::json!({
                "text": text,
                "author": author,
                "subtitle": subtitle,
                "bg": bg.peek().clone(),
                "ink": ink.peek().clone(),
                "ratio": ratio.peek().clone(),
                "font": font.peek().clone(),
                "size": size.peek().clone(),
                "bold": *bold.peek(),
                "italic": *italic.peek(),
                "filename": "omnibus-quote",
            })
            .to_string()
        }
    };
    // Runs the canvas `<a download>` (desktop). The phone actions below route
    // the same canvas through the OS share sheet / clipboard instead —
    // WKWebView's programmatic-download support is spotty.
    let download = {
        #[cfg(any(feature = "web", feature = "mobile"))]
        let build_payload = build_payload.clone();
        move |_| {
            #[cfg(any(feature = "web", feature = "mobile"))]
            quote_card_call("exportQuoteCard", &build_payload());
        }
    };
    let share = {
        #[cfg(any(feature = "web", feature = "mobile"))]
        let build_payload = build_payload.clone();
        move |_| {
            #[cfg(any(feature = "web", feature = "mobile"))]
            quote_card_call("shareQuoteCard", &build_payload());
        }
    };
    let copy_image = {
        #[cfg(any(feature = "web", feature = "mobile"))]
        let build_payload = build_payload.clone();
        move |_| {
            #[cfg(any(feature = "web", feature = "mobile"))]
            quote_card_call("copyQuoteCardImage", &build_payload());
        }
    };

    rsx! {
        {render_quote_header(on_close)}
        div { class: "rd-drawer-body",
            {render_quote_preview(&card_style, &body_style, &quote_text, &author, &subtitle)}
            div { class: "rd-quote-controls",
                {render_background_swatches(&accent, bg, ink, custom)}
                {render_font_controls(font)}
                {render_size_controls(size)}
                {render_style_controls(bold, italic)}
                {render_ratio_controls(ratio, &cur_ratio)}
                {render_quote_actions(download, copy_image, share)}
            }
        }
    }
}

// Every test here renders SSR markup, so the module is `server`-gated —
// under `web` its contents would be dead code and CI lints with `-D warnings`.
#[cfg(all(test, feature = "server"))]
mod tests;
