//! Mobile bottom tab bar.
//!
//! Pinned to the bottom of the viewport on the native shell, linking to the
//! library, authors, series, and settings routes. Mounted by
//! [`crate::ScreenLayout`] on the mobile target only.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

#[component]
pub fn BottomNav() -> Element {
    rsx! {
        nav { class: "bottom-nav",
            Link { to: Route::Landing {}, "Home" }
            Link { to: Route::AuthorsIndex {}, "Authors" }
            Link { to: Route::SeriesIndex {}, "Series" }
            Link { to: Route::Settings {}, "Settings" }
        }
    }
}
