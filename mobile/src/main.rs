//! Omnibus mobile — thin Dioxus Native shell.
//!
//! All UI lives in the `omnibus_frontend` crate under `features = ["mobile"]`.
//! This binary only wires platform launch, provides the hardcoded server URL
//! via context, hydrates the bearer-token store from disk on launch, and
//! delegates to the shared `App` component.

use dioxus::prelude::*;
use omnibus_frontend::{data::token_store, data::ServerUrl, App};

fn main() {
    // Pull any persisted bearer token into the in-memory store before the
    // first render so the initial API calls go out authenticated.
    //
    // TODO(F0.3 follow-up): the token is currently plaintext on disk.
    // Replace with iOS Keychain / Android Keystore before shipping to end
    // users — see the module docs on `omnibus_frontend::data::token_store`.
    token_store::load_from_disk();

    dioxus::launch(Root);
}

#[component]
fn Root() -> Element {
    use_context_provider(|| ServerUrl("http://127.0.0.1:3000".to_string()));
    rsx! { App {} }
}
