//! Omnibus mobile — thin native shell hosting a wry WebView.
//!
//! Dioxus `features = ["mobile"]` renders the shared `omnibus_frontend` rsx into
//! a system WebView (WKWebView on iOS, Android System WebView) via wry — not a
//! Blitz/native renderer. This binary only wires platform launch: seed the
//! persisted server-URL context, hydrate the bearer token from disk, and
//! delegate to the shared `App`.

use dioxus::prelude::*;
use omnibus_frontend::{
    data::{server_url_store, token_store, ServerUrl},
    App,
};

fn main() {
    // Pull any persisted bearer token into the in-memory store before the
    // first render so the initial API calls go out authenticated. On
    // iOS/desktop/Android this persists in every build (0o600 file under the
    // sandboxed app data dir), so users stay signed in across a cold start.
    //
    // TODO: harden at-rest storage with iOS Keychain / Android Keystore.
    // Current protection is the iOS sandbox / Android per-app UID sandbox +
    // 0o600 — see the module docs on `omnibus_frontend::data::token_store` /
    // `app_dirs`.
    token_store::load_from_disk();

    // Open the offline store (replica cache + outbox + download registry),
    // start the loopback media server, and hydrate cold-start state. Must
    // run before launch so first renders can serve cached data offline.
    omnibus_frontend::offline::init();

    // Paint the WebView itself in the dark theme's `--bg-0` so the cold
    // start never flashes the platform-default white. The first frame is
    // always Theme::Dark regardless of the persisted theme (see
    // `atrium::init_theme`), so this matches the first paint for everyone.
    dioxus::LaunchBuilder::new()
        .with_cfg(dioxus::mobile::Config::new().with_background_color((10, 8, 6, 255)))
        .launch(Root);
}

#[component]
fn Root() -> Element {
    // Seed the reactive server-URL signal from disk. Empty when nothing is
    // saved, which routes the user to the Connect screen on first launch.
    use_context_provider(|| ServerUrl(Signal::new(server_url_store::load().unwrap_or_default())));
    rsx! { App {} }
}
