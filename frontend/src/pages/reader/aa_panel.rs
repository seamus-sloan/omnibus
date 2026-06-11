//! Frosted-glass typography settings panel: theme switcher, typeface,
//! text size, line spacing, margins, and justify toggle.

use dioxus::prelude::*;

use crate::components::atrium::Theme;

use super::typography::{LineSpacing, Margins, Typeface};

/// Frosted-glass typography settings panel: theme switcher, typeface,
/// text size, line spacing, margins, and justify toggle.
#[component]
pub(crate) fn ReaderAaPanel(
    theme: Theme,
    font_pct: f32,
    typeface: Typeface,
    line_spacing: LineSpacing,
    margins: Margins,
    justify: bool,
    on_set_theme: EventHandler<Theme>,
    on_font_decrease: EventHandler<MouseEvent>,
    on_font_increase: EventHandler<MouseEvent>,
    on_set_typeface: EventHandler<Typeface>,
    on_set_line_spacing: EventHandler<LineSpacing>,
    on_set_margins: EventHandler<Margins>,
    on_toggle_justify: EventHandler<MouseEvent>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "rd-scrim", onclick: on_close }
        div {
            class: "rd-aa-panel",
            onclick: move |evt: MouseEvent| evt.stop_propagation(),

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Theme" }
                div {
                    class: "rd-seg",
                    button {
                        class: if theme == Theme::Dark { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_theme.call(Theme::Dark),
                        "Dark"
                    }
                    button {
                        class: if theme == Theme::Light { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_theme.call(Theme::Light),
                        "Light"
                    }
                    button {
                        class: if theme == Theme::Sepia { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_theme.call(Theme::Sepia),
                        "Sepia"
                    }
                }
            }

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Typeface" }
                div {
                    style: "display:flex; gap:6px;",
                    button {
                        class: if typeface == Typeface::Editorial { "rd-typeface-chip on" } else { "rd-typeface-chip" },
                        r#type: "button",
                        onclick: move |_| on_set_typeface.call(Typeface::Editorial),
                        span { class: "preview", style: "font-family:'Instrument Serif',serif;", "Aa" }
                        span { class: "name", "Editorial" }
                    }
                    button {
                        class: if typeface == Typeface::Classic { "rd-typeface-chip on" } else { "rd-typeface-chip" },
                        r#type: "button",
                        onclick: move |_| on_set_typeface.call(Typeface::Classic),
                        span { class: "preview", style: "font-family:'EB Garamond',serif;", "Aa" }
                        span { class: "name", "Classic" }
                    }
                    button {
                        class: if typeface == Typeface::Modern { "rd-typeface-chip on" } else { "rd-typeface-chip" },
                        r#type: "button",
                        onclick: move |_| on_set_typeface.call(Typeface::Modern),
                        span { class: "preview", style: "font-family:Georgia,serif;", "Aa" }
                        span { class: "name", "Modern" }
                    }
                }
            }

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Text size" }
                div {
                    style: "display:flex; align-items:center; gap:12px;",
                    button {
                        class: "rd-tool",
                        r#type: "button",
                        "aria-label": "Decrease font size",
                        "data-testid": "reader-font-decrease",
                        onclick: on_font_decrease,
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
                        onclick: on_font_increase,
                        style: "font-family:var(--serif); font-size:24px; color:var(--ink-1); min-width:24px; height:24px; padding:0;",
                        "A"
                    }
                }
            }

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Line spacing" }
                div {
                    class: "rd-seg",
                    button {
                        class: if line_spacing == LineSpacing::Tight { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_line_spacing.call(LineSpacing::Tight),
                        "Tight"
                    }
                    button {
                        class: if line_spacing == LineSpacing::Cozy { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_line_spacing.call(LineSpacing::Cozy),
                        "Cozy"
                    }
                    button {
                        class: if line_spacing == LineSpacing::Airy { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_line_spacing.call(LineSpacing::Airy),
                        "Airy"
                    }
                }
            }

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Margins" }
                div {
                    class: "rd-seg",
                    button {
                        class: if margins == Margins::Narrow { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_margins.call(Margins::Narrow),
                        "Narrow"
                    }
                    button {
                        class: if margins == Margins::Normal { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_margins.call(Margins::Normal),
                        "Normal"
                    }
                    button {
                        class: if margins == Margins::Wide { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_margins.call(Margins::Wide),
                        "Wide"
                    }
                }
            }

            div {
                class: "rd-toggle-row",
                span { style: "font-size:13px; color:var(--ink-1);", "Justify text" }
                div {
                    class: if justify { "rd-toggle-track on" } else { "rd-toggle-track" },
                    onclick: on_toggle_justify,
                    div { class: "rd-toggle-knob" }
                }
            }
        }
    }
}
