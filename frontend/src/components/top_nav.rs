//! Web top navigation bar.
//!
//! Brand link, primary section links (Library / Authors / Series), the
//! search-palette trigger, and the user menu. Mounted by
//! [`crate::ScreenLayout`] on every web route except the immersive reader.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route, Link};

use crate::components::search_palette::SearchPaletteTrigger;
use crate::components::user_menu::UserMenu;
use crate::pages::CheckInOpen;
use crate::Route;

/// Stoat brand mark shown next to the wordmark. Bundled + content-hashed by
/// manganis; served as a real URL so it hydrates identically SSR and WASM.
const BRAND_MARK: Asset = asset!("/assets/omnibus-stoat.png");

/// Renders the top navigation bar component.
#[component]
pub fn TopNav() -> Element {
    let route = use_route::<Route>();
    let nav = use_navigator();
    let mut check_in_open = use_context::<CheckInOpen>().0;
    // Hide the search trigger on `/settings` — the page has its own dense
    // form layout and a search button wedged into the nav above it just
    // clutters the chrome.
    let on_settings = matches!(route, Route::Settings { .. });
    let is_library = matches!(route, Route::Landing {});
    let is_authors = matches!(route, Route::AuthorsIndex {} | Route::AuthorDetail { .. });
    let is_series = matches!(route, Route::SeriesIndex {} | Route::SeriesDetail { .. });
    let is_stats = matches!(route, Route::Stats {});
    let can_upload = crate::use_can_upload();

    rsx! {
        nav { class: "atrium-topbar", aria_label: "Primary",
            Link {
                to: Route::Landing {},
                class: "atrium-brand",
                img { class: "atrium-brand-mark", src: BRAND_MARK, alt: "" }
                div { class: "atrium-brand-word", "Omnibus" }
            }
            div { class: "atrium-nav",
                Link {
                    to: Route::Landing {},
                    class: if is_library { "on" } else { "" },
                    "Library"
                }
                Link {
                    to: Route::AuthorsIndex {},
                    class: if is_authors { "on" } else { "" },
                    "Authors"
                }
                Link {
                    to: Route::SeriesIndex {},
                    class: if is_series { "on" } else { "" },
                    "Series"
                }
                Link {
                    to: Route::Stats {},
                    class: if is_stats { "on" } else { "" },
                    "Stats"
                }
            }
            if !on_settings {
                SearchPaletteTrigger {}
            }
            div { class: "atrium-actions",
                button {
                    class: "btn sm",
                    r#type: "button",
                    "data-testid": "check-in-button",
                    onclick: move |_| check_in_open.set(true),
                    "Check in"
                }
                AddBooksButton {
                    can_upload: can_upload(),
                    on_click: move |_| {
                        nav.push(Route::AddBooks {});
                    },
                }
                UserMenu {}
            }
        }
    }
}

/// The "Add books" nav entry, gated on `can_upload` — the server's
/// `require_upload` (`server/src/backend/uploads.rs`) is the real boundary;
/// this only keeps the entry point off a screen a non-uploader would 403 on.
/// Split out from [`TopNav`] so both render states are unit-testable without
/// a router/context (mirrors `SettingsSidebar`'s bool-prop shape in
/// `pages::settings`).
#[component]
fn AddBooksButton(can_upload: bool, on_click: EventHandler<MouseEvent>) -> Element {
    if !can_upload {
        return rsx! {};
    }
    rsx! {
        button {
            class: "btn primary sm",
            r#type: "button",
            "data-testid": "add-books-button",
            onclick: move |e| on_click.call(e),
            "Add books"
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    // `AddBooksButton` takes an `EventHandler` prop, and building one via
    // `Callback::new` asserts a Dioxus runtime is active — true inside a
    // component body mid-render, but not in a bare `rsx! { .. }` built
    // directly in a `#[test]` fn. So each harness constructs the button from
    // inside its own component body and the test drives it through a real
    // `VirtualDom`, mirroring `contexts::tests::AssertBustCounter`.
    #[component]
    fn AllowedHarness() -> Element {
        rsx! {
            AddBooksButton { can_upload: true, on_click: move |_| {} }
        }
    }

    #[component]
    fn ForbiddenHarness() -> Element {
        rsx! {
            AddBooksButton { can_upload: false, on_click: move |_| {} }
        }
    }

    #[test]
    fn add_books_button_omits_markup_when_can_upload_is_false() {
        let mut dom = VirtualDom::new(ForbiddenHarness);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        assert!(!html.contains("add-books-button"));
        assert!(!html.contains("Add books"));
    }

    #[test]
    fn add_books_button_renders_when_can_upload_is_true() {
        let mut dom = VirtualDom::new(AllowedHarness);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        assert!(html.contains("data-testid=\"add-books-button\""));
        assert!(html.contains("Add books"));
    }
}
