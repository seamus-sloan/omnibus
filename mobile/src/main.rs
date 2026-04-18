//! Omnibus mobile — thin Dioxus Native shell.
//!
//! All UI lives in the `omnibus_frontend` crate under `features = ["mobile"]`.
//! This binary only wires platform launch, provides the hardcoded server URL
//! via context, and delegates to the shared `App` component.

use dioxus::prelude::*;
use omnibus_frontend::{data::ServerUrl, App};

fn main() {
    dioxus::launch(Root);
}

#[component]
fn Root() -> Element {
    use_context_provider(|| ServerUrl("http://127.0.0.1:3000".to_string()));
    rsx! { App {} }
}
