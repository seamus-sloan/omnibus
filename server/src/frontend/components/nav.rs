use dioxus::prelude::*;

#[component]
pub fn TopNav() -> Element {
    rsx! {
        nav { class: "top-nav",
            a { href: "/", "Home" }
            a { href: "/settings", "Settings" }
        }
    }
}
