//! Shared scrim and shell chrome for the listen page's drawers and overlay
//! panels. [`ListenDrawerShell`] backs the bookmarks/chapters drawers
//! (`lp-drawer`: head with kicker and title, grouped actions, body);
//! [`ListenPanelShell`] backs the sleep/speed panels (the thinner `lp-panel`).
//! They differ only in outer class and close-button ownership.

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
/// (`lp-sleep-panel` / `lp-speed-panel`); `label` names the dialog for
/// assistive tech.
///
/// The panel takes focus once painted so Escape dismisses it without a prior
/// click — it has no close button of its own, and before this the only way
/// out was an unhinted click on empty space (#2242).
#[component]
pub(super) fn ListenPanelShell(
    extra_class: String,
    label: String,
    testid: String,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "lp-scrim", onclick: move |_| on_close.call(()) }
        div {
            class: "lp-panel {extra_class}",
            "data-testid": "{testid}",
            role: "dialog",
            "aria-label": "{label}",
            tabindex: "-1",
            onkeydown: move |evt: KeyboardEvent| {
                if evt.key() == Key::Escape {
                    evt.prevent_default();
                    on_close.call(());
                }
            },
            onmounted: move |evt: MountedEvent| crate::focus_after_paint::focus_after_paint(&evt),
            {children}
        }
    }
}

// Render-smoke coverage of the overlay panel's dismissal chrome — a separate
// module because SSR (`dioxus::ssr`) needs the `server` feature, and the
// harness runs inside a real `VirtualDom` because `EventHandler::new` needs a
// live runtime.
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn panel() -> Element {
        rsx! {
            ListenPanelShell {
                extra_class: "lp-speed-panel",
                label: "Playback speed",
                testid: "speed-panel",
                on_close: EventHandler::new(|()| {}),
                div { "body" }
            }
        }
    }

    // Regression for issue #2242 (AC1): the panel carries no close button, so
    // it has to be focusable and labelled for Escape to reach its keydown
    // handler without a prior click.
    #[test]
    fn panel_shell_renders_a_focusable_labelled_dialog() {
        let html = render_in_vdom(panel);
        assert!(html.contains(r#"role="dialog""#), "{html}");
        assert!(html.contains(r#"aria-label="Playback speed""#), "{html}");
        assert!(html.contains(r#"tabindex="-1""#), "{html}");
        assert!(html.contains(r#"data-testid="speed-panel""#), "{html}");
    }
}
