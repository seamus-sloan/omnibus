//! "Empty after filtering" placeholder for the landing grid.
//!
//! The facet sidebar + format chips were retired when shelves became the
//! library's primary lens (F3.1); only the empty-state affordance the
//! search path still hits remains here.

use dioxus::prelude::*;

#[component]
pub(super) fn EmptyFiltered(on_clear: EventHandler<()>) -> Element {
    rsx! {
        div { class: "library-empty",
            p { "No books match these filters." }
            button {
                class: "btn",
                "data-testid": "lib-clear-filters-empty",
                onclick: move |_| on_clear.call(()),
                "Clear filters"
            }
        }
    }
}
