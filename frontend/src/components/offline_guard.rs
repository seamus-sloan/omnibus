//! App-level "you're offline" sheet (mobile). When a reader/player route is
//! opened for a book with no completed local download, the page bounces back
//! to where the user came from and raises this sheet via [`block`] — instead
//! of stranding them inside a surface that can never load. State rides a
//! static `tokio::sync::watch` channel (the `token_store` subscriber house
//! pattern) so the setter needs no Dioxus context.

use std::sync::OnceLock;

use dioxus::prelude::*;
use tokio::sync::watch;

type Channel = (
    watch::Sender<Option<String>>,
    watch::Receiver<Option<String>>,
);

fn channel() -> &'static Channel {
    static CH: OnceLock<Channel> = OnceLock::new();
    CH.get_or_init(|| watch::channel(None))
}

/// Raise the sheet with `message`. Safe to call from any page.
pub(crate) fn block(message: &str) {
    let msg = message.to_string();
    channel().0.send_modify(|m| *m = Some(msg));
}

fn dismiss() {
    channel().0.send_modify(|m| *m = None);
}

/// The sheet itself — mounted once in the mobile `ScreenLayout`. Renders
/// nothing until a page calls [`block`].
#[component]
pub fn OfflineGuardModal() -> Element {
    let mut message = use_signal(|| None::<String>);
    use_future(move || async move {
        let mut rx = channel().0.subscribe();
        loop {
            // Sync before awaiting: a `block` racing this future's start (the
            // bouncing page sets it during the same navigation) must not be
            // missed.
            let now = rx.borrow_and_update().clone();
            if now != message() {
                message.set(now);
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });

    let Some(msg) = message() else {
        return rsx! {};
    };
    rsx! {
        div {
            class: "m-sheet-scrim",
            "data-testid": "offline-guard",
            onclick: move |_| dismiss(),
            div { class: "m-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "m-sheet-grabber" }
                div { class: "m-sheet-head",
                    h4 { "You\u{2019}re offline" }
                }
                p { class: "subtitle m-offline-guard-msg", role: "alert", "{msg}" }
                button {
                    r#type: "button",
                    class: "btn primary m-sheet-show",
                    onclick: move |_| dismiss(),
                    "OK"
                }
            }
        }
    }
}
