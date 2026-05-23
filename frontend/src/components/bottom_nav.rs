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
