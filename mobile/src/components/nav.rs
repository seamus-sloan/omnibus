use dioxus::prelude::*;
use dioxus_router::{Link, Routable};

use crate::Route;

#[component]
pub fn BottomNav() -> Element {
    rsx! {
        nav { class: "bottom-nav",
            Link { to: Route::Landing {}, "Home" }
            Link { to: Route::Settings {}, "Settings" }
        }
    }
}
