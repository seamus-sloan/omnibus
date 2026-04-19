use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

#[component]
pub fn bottom_nav() -> Element {
    rsx! {
        nav { class: "bottom-nav",
            Link { to: Route::Landing {}, "Home" }
            Link { to: Route::Settings {}, "Settings" }
        }
    }
}
