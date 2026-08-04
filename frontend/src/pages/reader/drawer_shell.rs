//! Shared scrim + slide-up drawer chrome for the reader's TOC, highlights,
//! search, and bookmarks panels. Each drawer differs only in its head content
//! (title, optional count/sub/actions) and body; this owns the scrim, the
//! `rd-drawer` shell, the grabber, and the close button all four repeated.

use dioxus::prelude::*;

/// Scrim + `rd-drawer` shell: grabber, a `head` slot (title/count/sub),
/// an `actions` slot grouped with the close button (e.g. the bookmarks
/// drawer's "+ Bookmark"), then `children` for the drawer's body/extra
/// sections. `extra_class` appends additional drawer classes (the search
/// panel's `rd-search-drawer`, which lets the phone breakpoint take it
/// full-screen).
#[component]
pub(super) fn ReaderDrawerShell(
    testid: String,
    #[props(default)] extra_class: String,
    on_close: EventHandler<()>,
    head: Element,
    #[props(default)] actions: Option<Element>,
    children: Element,
) -> Element {
    let drawer_class = if extra_class.is_empty() {
        "rd-drawer".to_string()
    } else {
        format!("rd-drawer {extra_class}")
    };
    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "{drawer_class}", "data-testid": "{testid}",
            div { class: "rd-grabber" }
            div { class: "rd-drawer-head",
                {head}
                div { class: "rd-drawer-head-actions",
                    {actions.unwrap_or_else(|| rsx! {})}
                    button {
                        class: "rd-x",
                        r#type: "button",
                        "aria-label": "Close",
                        onclick: move |_| on_close.call(()),
                        "\u{00d7}"
                    }
                }
            }
            {children}
        }
    }
}
