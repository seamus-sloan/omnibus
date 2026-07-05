//! Omnibus mobile — thin Dioxus Native shell.
//!
//! All UI lives in the `omnibus_frontend` crate under `features = ["mobile"]`.
//! This binary only wires platform launch, seeds the reactive server-URL
//! context from persisted state (empty on first run → the pre-login Connect
//! screen), hydrates the bearer-token store from disk on launch, and
//! delegates to the shared `App` component.

use dioxus::prelude::*;
use omnibus_frontend::{
    data::{server_url_store, token_store, ServerUrl},
    App,
};

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
    // Seed the reactive server-URL signal from disk. Empty when nothing is
    // saved, which routes the user to the Connect screen on first launch.
    use_context_provider(|| ServerUrl(Signal::new(server_url_store::load().unwrap_or_default())));
    rsx! { App {} }
}
