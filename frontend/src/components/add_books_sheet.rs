//! "Add to your library" bottom sheet — the chooser opened by the bottom
//! nav's center action. Icon + title + subtitle rows route into the existing
//! check-in and add-books surfaces. Rendered on every target; the closed
//! state is an absent node so SSR and the first client paint agree.

use dioxus::prelude::*;
use dioxus_router::use_navigator;

use crate::pages::CheckInOpen;
use crate::Route;

/// What a chooser row does when tapped: navigate to a route, or raise the
/// centered check-in overlay (which has no route of its own).
#[derive(Clone)]
enum AddAction {
    Navigate(Route),
    CheckIn,
}

/// One chooser row: what it does, its highlight state, and its copy.
struct AddRow {
    action: AddAction,
    primary: bool,
    title: &'static str,
    subtitle: &'static str,
    icon: Element,
}

/// The add-to-library chooser sheet. Renders nothing when `open` is false, so
/// the first paint (closed) is identical SSR and client.
#[component]
pub(super) fn AddBooksSheet(open: Signal<bool>, on_close: EventHandler<()>) -> Element {
    if !open() {
        return rsx!();
    }
    let nav = use_navigator();
    let mut check_in_open = use_context::<CheckInOpen>().0;
    let rows = vec![
        AddRow {
            action: AddAction::CheckIn,
            primary: true,
            title: "Scan a barcode",
            subtitle: "Check in a book you own",
            icon: icon_scan(),
        },
        AddRow {
            action: AddAction::Navigate(Route::AddBooks {}),
            primary: false,
            title: "Upload a file",
            subtitle: "Add an EPUB from this device",
            icon: icon_upload(),
        },
        AddRow {
            action: AddAction::CheckIn,
            primary: false,
            title: "Enter an ISBN",
            subtitle: "Add a book by its number",
            icon: icon_isbn(),
        },
    ];

    rsx! {
        div {
            class: "m-sheet-scrim",
            "data-testid": "add-books-sheet-scrim",
            onclick: move |_| on_close.call(()),
            div {
                class: "m-sheet",
                "data-testid": "add-books-sheet",
                onclick: move |e| e.stop_propagation(),
                div { class: "m-sheet-grabber" }
                div { class: "m-sheet-head",
                    h4 { "Add to your library" }
                }
                div { class: "m-sheet-list",
                    for row in rows {
                        {
                            let AddRow { action, primary, title, subtitle, icon } = row;
                            rsx! {
                                button {
                                    key: "{title}",
                                    r#type: "button",
                                    class: if primary { "m-add-row m-add-row--primary" } else { "m-add-row" },
                                    "data-testid": "add-books-row",
                                    onclick: move |_| {
                                        on_close.call(());
                                        match &action {
                                            AddAction::Navigate(route) => {
                                                nav.push(route.clone());
                                            }
                                            AddAction::CheckIn => check_in_open.set(true),
                                        }
                                    },
                                    span { class: "m-add-icon", {icon} }
                                    span { class: "m-add-body",
                                        span { class: "m-add-title", "{title}" }
                                        span { class: "m-add-sub", "{subtitle}" }
                                    }
                                    span { class: "m-add-chevron", "\u{203a}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Barcode-scan glyph for the highlighted Scan row.
fn icon_scan() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M3 7V5a2 2 0 0 1 2-2h2" }
            path { d: "M17 3h2a2 2 0 0 1 2 2v2" }
            path { d: "M21 17v2a2 2 0 0 1-2 2h-2" }
            path { d: "M7 21H5a2 2 0 0 1-2-2v-2" }
            path { d: "M7 8v8" }
            path { d: "M11 8v8" }
            path { d: "M15 8v8" }
        }
    }
}

/// Upload-arrow glyph, mirroring the add-books drop zone.
fn icon_upload() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
            polyline { points: "17 8 12 3 7 8" }
            line { x1: "12", y1: "3", x2: "12", y2: "15" }
        }
    }
}

/// Keypad/number glyph for the manual-ISBN row.
fn icon_isbn() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            line { x1: "4", y1: "9", x2: "20", y2: "9" }
            line { x1: "4", y1: "15", x2: "20", y2: "15" }
            line { x1: "10", y1: "3", x2: "8", y2: "21" }
            line { x1: "16", y1: "3", x2: "14", y2: "21" }
        }
    }
}
