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

#[component]
pub(super) fn QuotePanel(
    quote_text: String,
    author: String,
    subtitle: String,
    accent: String,
    on_close: EventHandler<()>,
) -> Element {
    let mut bg = use_signal(|| accent.clone());
    let mut ink = use_signal(|| "#f5f0e8".to_string());
    let mut ratio = use_signal(|| "1:1".to_string());
    let mut custom = use_signal(|| "#6b4f8a".to_string());

    let cur_bg = bg();
    let cur_ink = ink();
    let cur_ratio = ratio();
    let preview_style = format!(
        "aspect-ratio:{}; background:{cur_bg}; color:{cur_ink};",
        cur_ratio.replace(':', "/")
    );

    let download = {
        let dl_text = quote_text.clone();
        let dl_author = author.clone();
        let dl_subtitle = subtitle.clone();
        move |_| {
            #[cfg(feature = "web")]
            {
                let payload = serde_json::json!({
                    "text": dl_text,
                    "author": dl_author,
                    "subtitle": dl_subtitle,
                    "bg": bg.peek().clone(),
                    "ink": ink.peek().clone(),
                    "ratio": ratio.peek().clone(),
                    "filename": "omnibus-quote",
                })
                .to_string();
                let arg = serde_json::to_string(&payload).unwrap_or_else(|_| "\"{}\"".into());
                super::reader_call("exportQuoteCard", &arg);
            }
            #[cfg(not(feature = "web"))]
            let _ = (&dl_text, &dl_author, &dl_subtitle);
        }
    };

    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "rd-drawer rd-quote-drawer", "data-testid": "reader-quote-drawer",
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
            div { class: "rd-drawer-body",
                div { class: "rd-quote-card", style: "{preview_style}",
                    div { class: "rd-quote-card-head", "OMNIBUS \u{00b7} QUOTE" }
                    div { class: "rd-quote-card-body", "\u{201c}{quote_text}\u{201d}" }
                    div { class: "rd-quote-card-foot",
                        div { class: "rd-quote-card-author", "{author}" }
                        div { class: "rd-quote-card-sub", "{subtitle}" }
                    }
                }

                div { class: "rd-quote-controls",
                    div { class: "rd-quote-label", "Background" }
                    div { class: "rd-quote-swatches",
                        button {
                            class: "rd-quote-swatch",
                            r#type: "button",
                            style: "background:{accent};",
                            "aria-label": "Book accent",
                            onclick: {
                                let accent = accent.clone();
                                move |_| {
                                    bg.set(accent.clone());
                                    ink.set("#f5f0e8".into());
                                }
                            },
                        }
                        for p in PRESETS.iter() {
                            button {
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

                    div { class: "rd-quote-label", "Aspect" }
                    div { class: "rd-quote-ratios",
                        for r in RATIOS {
                            button {
                                class: if cur_ratio == r { "rd-chip on" } else { "rd-chip" },
                                r#type: "button",
                                onclick: move |_| ratio.set(r.to_string()),
                                "{r}"
                            }
                        }
                    }

                    div { class: "rd-quote-actions",
                        button {
                            class: "btn primary",
                            r#type: "button",
                            "data-testid": "quote-download",
                            onclick: download,
                            "Download PNG"
                        }
                        button {
                            class: "btn",
                            r#type: "button",
                            disabled: true,
                            title: "Journal & quote cards ship in a later phase",
                            "Open in composer \u{2192}"
                        }
                    }
                }
            }
        }
    }
}
