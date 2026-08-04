//! Shared scrim + shell chrome for the listen page's drawers and overlay
//! panels — [`ListenDrawerShell`] backs the bookmarks/chapters drawers
//! (`lp-drawer`: head with kicker/title, grouped trailing actions, body),
//! [`ListenPanelShell`] backs the sleep/speed overlay panels (the thinner
//! `lp-panel` wrapper). Both differ from the other only in their outer class
//! and whether they own a close button, so this stays two small components
//! rather than one over-parameterized shape.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

/// Scrim + `lp-drawer` shell: a head slot (kicker/title block), a trailing
/// actions slot grouped with the close button, then `children` for the
/// drawer body.
#[component]
pub(super) fn ListenDrawerShell(
    testid: String,
    on_close: EventHandler<()>,
    head: Element,
    #[props(default)] actions: Option<Element>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "lp-scrim", onclick: move |_| on_close.call(()) }
        div { class: "lp-drawer", "data-testid": "{testid}",
            div { class: "lp-drawer-head",
                {head}
                div { class: "lp-drawer-head-actions",
                    {actions.unwrap_or_else(|| rsx! {})}
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        "Close \u{2193}"
                    }
                }
            }
            div { class: "lp-drawer-body", {children} }
        }
    }
}

/// Scrim + `lp-panel` shell around an overlay panel's body (sleep timer,
/// playback speed). `extra_class` appends the panel's own modifier class
/// (`lp-sleep-panel` / `lp-speed-panel`); `testid` is optional since only the
/// sleep panel carries one today.
#[component]
pub(super) fn ListenPanelShell(
    extra_class: String,
    #[props(default)] testid: Option<String>,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "lp-scrim", onclick: move |_| on_close.call(()) }
        div { class: "lp-panel {extra_class}", "data-testid": testid, {children} }
    }
}
