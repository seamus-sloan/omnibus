//! Quote-card panel. Turns a highlight into a shareable card: live preview,
//! background presets + custom color, aspect-ratio choice, and a PNG export
//! drawn by the glue's `exportQuoteCard` (a bespoke canvas renderer).

use dioxus::prelude::*;

/// A background/ink preset swatch.
struct Preset {
    bg: &'static str,
    ink: &'static str,
}

const RATIOS: [&str; 4] = ["1:1", "4:5", "9:16", "3:4"];

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

/// Drawer head: kicker + title + close button.
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

/// The live quote-card preview, styled with the current background/ink/ratio.
fn render_quote_preview(
    preview_style: &str,
    quote_text: &str,
    author: &str,
    subtitle: &str,
) -> Element {
    rsx! {
        div { class: "rd-quote-card", style: "{preview_style}",
            div { class: "rd-quote-card-head", "OMNIBUS \u{00b7} QUOTE" }
            div { class: "rd-quote-card-body", "\u{201c}{quote_text}\u{201d}" }
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
            label { class: "rd-quote-custom",
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

/// Aspect-ratio chip row.
fn render_ratio_controls(ratio: Signal<String>, cur_ratio: &str) -> Element {
    let mut ratio = ratio;
    rsx! {
        div { class: "rd-quote-label", "Aspect" }
        div { class: "rd-quote-ratios",
            for r in RATIOS {
                button {
                    key: "{r}",
                    class: if cur_ratio == r { "rd-chip on" } else { "rd-chip" },
                    r#type: "button",
                    onclick: move |_| ratio.set(r.to_string()),
                    "{r}"
                }
            }
        }
    }
}

/// Export actions: download (desktop), copy/share (phone), composer (disabled).
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
                "Download PNG"
            }
            button {
                class: "btn rd-phone-only",
                r#type: "button",
                "data-testid": "quote-copy-image",
                onclick: copy_image,
                "Copy image"
            }
            // Phone primary action: the OS share sheet takes the
            // rendered PNG (glue falls back to a download where
            // Web Share can't carry files).
            button {
                class: "btn primary rd-phone-only",
                r#type: "button",
                "data-testid": "quote-share",
                onclick: share,
                "Share"
            }
            button {
                class: "btn rd-desktop-only",
                r#type: "button",
                disabled: true,
                title: "Journal & quote cards ship in a later phase",
                "Open in composer \u{2192}"
            }
        }
    }
}

#[component]
pub(super) fn QuotePanel(
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

    let cur_bg = bg();
    let cur_ink = ink();
    let cur_ratio = ratio();
    let preview_style = format!(
        "aspect-ratio:{}; background:{cur_bg}; color:{cur_ink};",
        cur_ratio.replace(':', "/")
    );

    // One canvas payload for all three glue actions (download / share /
    // copy); `Clone` so each button handler owns a copy. Only the interactive
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
                "filename": "omnibus-quote",
            })
            .to_string()
        }
    };
    // Runs the glue's canvas `<a download>` (desktop). The phone actions below
    // route the same canvas through the OS share sheet / clipboard instead —
    // WKWebView's programmatic-download support is spotty.
    let download = {
        #[cfg(any(feature = "web", feature = "mobile"))]
        let build_payload = build_payload.clone();
        move |_| {
            #[cfg(any(feature = "web", feature = "mobile"))]
            super::reader_call_json("exportQuoteCard", &build_payload());
        }
    };
    let share = {
        #[cfg(any(feature = "web", feature = "mobile"))]
        let build_payload = build_payload.clone();
        move |_| {
            #[cfg(any(feature = "web", feature = "mobile"))]
            super::reader_call_json("shareQuoteCard", &build_payload());
        }
    };
    let copy_image = {
        #[cfg(any(feature = "web", feature = "mobile"))]
        let build_payload = build_payload.clone();
        move |_| {
            #[cfg(any(feature = "web", feature = "mobile"))]
            super::reader_call_json("copyQuoteCardImage", &build_payload());
        }
    };

    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "rd-drawer rd-quote-drawer", "data-testid": "reader-quote-drawer",
            div { class: "rd-grabber" }
            {render_quote_header(on_close)}
            div { class: "rd-drawer-body",
                {render_quote_preview(&preview_style, &quote_text, &author, &subtitle)}
                div { class: "rd-quote-controls",
                    {render_background_swatches(&accent, bg, ink, custom)}
                    {render_ratio_controls(ratio, &cur_ratio)}
                    {render_quote_actions(download, copy_image, share)}
                }
            }
        }
    }
}
