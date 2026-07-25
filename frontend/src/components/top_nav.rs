//! Web top navigation bar.
//!
//! Brand link, primary section links (Library / Authors / Series), the
//! search-palette trigger, and the user menu. Mounted by
//! [`crate::ScreenLayout`] on every web route except the immersive reader.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route, Link};

use crate::components::search_palette::SearchPaletteTrigger;
use crate::components::user_menu::UserMenu;
use crate::Route;

/// Stoat brand mark shown next to the wordmark. Bundled + content-hashed by
/// manganis; served as a real URL so it hydrates identically SSR and WASM.
const BRAND_MARK: Asset = asset!("/assets/omnibus-stoat.png");

/// Renders the top navigation bar component.
#[component]
pub fn TopNav() -> Element {
    let route = use_route::<Route>();
    let nav = use_navigator();
    // Hide the search trigger on `/settings` — the page has its own dense
    // form layout and a search button wedged into the nav above it just
    // clutters the chrome.
    let on_settings = matches!(route, Route::Settings {});
    let is_library = matches!(route, Route::Landing {});
    let is_authors = matches!(route, Route::AuthorsIndex {} | Route::AuthorDetail { .. });
    let is_series = matches!(route, Route::SeriesIndex {} | Route::SeriesDetail { .. });
    let is_stats = matches!(route, Route::Stats {});

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
                    onclick: move |_| {
                        nav.push(Route::CheckIn {});
                    },
                    "Check in"
                }
                button {
                    class: "btn primary sm",
                    r#type: "button",
                    "data-testid": "add-books-button",
                    onclick: move |_| {
                        nav.push(Route::AddBooks {});
                    },
                    "Add books"
                }
                UserMenu {}
            }
        }
    }
}
