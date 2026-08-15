//! Render-smoke coverage for [`UmStat`] and [`UmSessionRows`] (#1913): a
//! stubbed row/tile must render as a plain, non-interactive element with no
//! `href` at all, and no row — linked or not — may ever emit `href="#"`.
//! The `to: Some(route)` tile's `Link` needs a live `RouterContext`, which
//! (like `shelves_rail`'s "All books" row, see its `tests.rs`) this harness
//! doesn't mount; Dioxus catches that per-component, so the surrounding
//! grid still renders and the `href="#"` regression check still holds
//! across every tile/row.

use super::*;
use crate::test_support::render_in_vdom;

fn stat_grid_harness() -> Element {
    let open = use_signal(|| false);
    rsx! {
        div { class: "um-stat-grid",
            UmStat { label: "Journal", detail: "24 entries", to: None, open }
            UmStat {
                label: "Shelves",
                detail: "3 shared",
                to: Some(Route::Shelves {}),
                open,
            }
        }
    }
}

/// Isolates the `to: None` tile alone (no sibling `Link`) so the
/// href-absence assertion below is unambiguous about which node it covers.
fn static_stat_harness() -> Element {
    let open = use_signal(|| false);
    rsx! {
        UmStat { label: "Journal", detail: "24 entries", to: None, open }
    }
}

#[test]
fn um_stat_renders_a_non_interactive_div_when_no_destination_exists() {
    let html = render_in_vdom(static_stat_harness);
    assert!(html.contains("um-stat-static"));
    assert!(html.contains("Journal"));
    assert!(html.contains("24 entries"));
    assert!(html.contains("aria-disabled=\"true\""));
    assert!(
        !html.contains("href"),
        "a non-interactive stat tile must not be an anchor at all, got: {html}"
    );
}

#[test]
fn um_stat_never_emits_an_href_hash_placeholder() {
    let html = render_in_vdom(stat_grid_harness);
    assert!(
        !html.contains("href=\"#\""),
        "a stat tile must never render a dead href=\"#\" link (#1913), got: {html}"
    );
}

fn session_rows_harness() -> Element {
    rsx! {
        UmSessionRows { on_signout: EventHandler::new(|()| {}) }
    }
}

#[test]
fn um_session_rows_renders_switch_user_as_a_non_interactive_row() {
    let html = render_in_vdom(session_rows_harness);
    assert!(html.contains("Switch user"));
    assert!(html.contains("aria-disabled=\"true\""));
    assert!(
        !html.contains("href"),
        "the stubbed Switch-user row must not be an anchor at all (#1913), got: {html}"
    );
}
