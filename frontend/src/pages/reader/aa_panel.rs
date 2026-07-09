//! Frosted-glass typography settings panel: theme switcher, typeface,
//! text size, line spacing, margins, and justify toggle. Reads the
//! current preference set from the `ReaderPrefs` context published by
//! `BookReadPage`, so the panel takes no preference / handler props.

use dioxus::prelude::*;

use crate::components::atrium::Theme;

use super::prefs::ReaderPrefs;
use super::typography::{LineSpacing, Margins, Spread, Typeface};

/// Frosted-glass typography settings panel: theme switcher, typeface,
/// text size, line spacing, margins, and justify toggle.
#[component]
pub(crate) fn ReaderAaPanel(on_close: EventHandler<MouseEvent>) -> Element {
    rsx! {
        div { class: "rd-scrim", onclick: on_close }
        div {
            class: "rd-aa-panel",
            onclick: move |evt: MouseEvent| evt.stop_propagation(),

            div { class: "rd-grabber" }
            ThemeRow {}
            TypefaceRow {}
            TextSizeRow {}
            LineSpacingRow {}
            MarginsRow {}
            PageViewRow {}
            JustifyRow {}
        }
    }
}

/// Theme segmented control (Dark / Light / Sepia).
#[component]
fn ThemeRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let theme = *prefs.theme.read();
    rsx! {
        div { class: "rd-aa-row",
            div { class: "rd-aa-label", "Theme" }
            div {
                class: "rd-seg",
                button {
                    class: if theme == Theme::Dark { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_theme(Theme::Dark),
                    "Dark"
                }
                button {
                    class: if theme == Theme::Light { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_theme(Theme::Light),
                    "Light"
                }
                button {
                    class: if theme == Theme::Sepia { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_theme(Theme::Sepia),
                    "Sepia"
                }
            }
        }
    }
}

/// Typeface chip row (Editorial / Classic / Modern).
#[component]
fn TypefaceRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let typeface = *prefs.typeface.read();
    rsx! {
        div { class: "rd-aa-row",
            div { class: "rd-aa-label", "Typeface" }
            div {
                style: "display:flex; gap:6px;",
                button {
                    class: if typeface == Typeface::Editorial { "rd-typeface-chip on" } else { "rd-typeface-chip" },
                    r#type: "button",
                    onclick: move |_| prefs.set_typeface(Typeface::Editorial),
                    span { class: "preview", style: "font-family:'Instrument Serif',serif;", "Aa" }
                    span { class: "name", "Editorial" }
                }
                button {
                    class: if typeface == Typeface::Classic { "rd-typeface-chip on" } else { "rd-typeface-chip" },
                    r#type: "button",
                    onclick: move |_| prefs.set_typeface(Typeface::Classic),
                    span { class: "preview", style: "font-family:'EB Garamond',serif;", "Aa" }
                    span { class: "name", "Classic" }
                }
                button {
                    class: if typeface == Typeface::Modern { "rd-typeface-chip on" } else { "rd-typeface-chip" },
                    r#type: "button",
                    onclick: move |_| prefs.set_typeface(Typeface::Modern),
                    span { class: "preview", style: "font-family:Georgia,serif;", "Aa" }
                    span { class: "name", "Modern" }
                }
            }
        }
    }
}

/// Text-size stepper with a fill track showing the current font percentage.
#[component]
fn TextSizeRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let font_pct = prefs.font_pct();
    rsx! {
        div { class: "rd-aa-row",
            div { class: "rd-aa-label", "Text size" }
            div {
                style: "display:flex; align-items:center; gap:12px;",
                button {
                    class: "rd-tool",
                    r#type: "button",
                    "aria-label": "Decrease font size",
                    "data-testid": "reader-font-decrease",
                    onclick: move |_| prefs.decrease_font(),
                    style: "font-family:var(--serif); font-size:13px; color:var(--ink-2); min-width:24px; height:24px; padding:0;",
                    "A"
                }
                div {
                    class: "rd-size-track",
                    div { class: "rd-size-fill", style: "width:{font_pct}%;" }
                    div { class: "rd-size-thumb", style: "left:{font_pct}%;" }
                }
                button {
                    class: "rd-tool",
                    r#type: "button",
                    "aria-label": "Increase font size",
                    "data-testid": "reader-font-increase",
                    onclick: move |_| prefs.increase_font(),
                    style: "font-family:var(--serif); font-size:24px; color:var(--ink-1); min-width:24px; height:24px; padding:0;",
                    "A"
                }
            }
        }
    }
}

/// Line-spacing segmented control (Tight / Cozy / Airy).
#[component]
fn LineSpacingRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let line_spacing = *prefs.line_spacing.read();
    rsx! {
        div { class: "rd-aa-row",
            div { class: "rd-aa-label", "Line spacing" }
            div {
                class: "rd-seg",
                button {
                    class: if line_spacing == LineSpacing::Tight { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_line_spacing(LineSpacing::Tight),
                    "Tight"
                }
                button {
                    class: if line_spacing == LineSpacing::Cozy { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_line_spacing(LineSpacing::Cozy),
                    "Cozy"
                }
                button {
                    class: if line_spacing == LineSpacing::Airy { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_line_spacing(LineSpacing::Airy),
                    "Airy"
                }
            }
        }
    }
}

/// Margins segmented control (Narrow / Normal / Wide).
#[component]
fn MarginsRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let margins = *prefs.margins.read();
    rsx! {
        div { class: "rd-aa-row",
            div { class: "rd-aa-label", "Margins" }
            div {
                class: "rd-seg",
                button {
                    class: if margins == Margins::Narrow { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_margins(Margins::Narrow),
                    "Narrow"
                }
                button {
                    class: if margins == Margins::Normal { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_margins(Margins::Normal),
                    "Normal"
                }
                button {
                    class: if margins == Margins::Wide { "on" } else { "" },
                    r#type: "button",
                    onclick: move |_| prefs.set_margins(Margins::Wide),
                    "Wide"
                }
            }
        }
    }
}

/// Page-view segmented control (Single / Two-page spread).
#[component]
fn PageViewRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let spread = *prefs.spread.read();
    rsx! {
        // `rd-aa-row-spread` lets the phone breakpoint hide this row — a
        // phone column never renders a two-page spread.
        div { class: "rd-aa-row rd-aa-row-spread",
            div { class: "rd-aa-label", "Page view" }
            div {
                class: "rd-seg",
                button {
                    class: if spread == Spread::Single { "on" } else { "" },
                    r#type: "button",
                    "data-testid": "reader-spread-single",
                    onclick: move |_| prefs.set_spread(Spread::Single),
                    "Single"
                }
                button {
                    class: if spread == Spread::Double { "on" } else { "" },
                    r#type: "button",
                    "data-testid": "reader-spread-double",
                    onclick: move |_| prefs.set_spread(Spread::Double),
                    "Two-page"
                }
            }
        }
    }
}

/// Justify-text toggle switch.
#[component]
fn JustifyRow() -> Element {
    let prefs = use_context::<ReaderPrefs>();
    let justify = *prefs.justify.read();
    rsx! {
        div {
            class: "rd-toggle-row",
            span { style: "font-size:13px; color:var(--ink-1);", "Justify text" }
            div {
                class: if justify { "rd-toggle-track on" } else { "rd-toggle-track" },
                onclick: move |_| prefs.toggle_justify(),
                div { class: "rd-toggle-knob" }
            }
        }
    }
}
