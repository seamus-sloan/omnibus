use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

#[component]
pub fn TopNav() -> Element {
    rsx! {
        nav { class: "top-nav",
            Link { to: Route::Landing {}, "Home" }
            Link { to: Route::Settings {}, "Settings" }
        }
    }
}
