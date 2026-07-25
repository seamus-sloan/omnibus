//! Mobile connectivity pill: a small fixed chip above the bottom nav that
//! appears while the app is offline (with the count of queued changes) or
//! while queued changes are still syncing after reconnect. Hidden entirely
//! in the steady online state. Driven by the offline sync engine's watch
//! channel (never by polling).

use dioxus::prelude::*;

use crate::offline::sync::{self, NetState};

/// The pill. Mounted once by the mobile `ScreenLayout`.
#[component]
pub fn OfflinePill() -> Element {
    let mut net = use_signal(sync::state);
    use_future(move || async move {
        let mut rx = sync::subscribe();
        loop {
            let now = *rx.borrow_and_update();
            if now != net() {
                net.set(now);
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });

    let state = net();
    let Some(label) = pill_label(&state) else {
        return rsx! {};
    };
    let class = if state.online {
        "m-offline-pill is-syncing"
    } else {
        "m-offline-pill"
    };
    rsx! {
        div { class: "{class}", role: "status", "data-testid": "offline-pill",
            span { class: "m-offline-pill-dot", "aria-hidden": "true" }
            "{label}"
        }
    }
}

/// The pill text for a state, `None` when the pill should hide (online and
/// nothing pending).
fn pill_label(state: &NetState) -> Option<String> {
    match (state.online, state.pending_ops) {
        (true, 0) => None,
        (true, n) => Some(format!("Syncing \u{00b7} {n} pending")),
        (false, 0) => Some("Offline".to_string()),
        (false, 1) => Some("Offline \u{00b7} 1 change queued".to_string()),
        (false, n) => Some(format!("Offline \u{00b7} {n} changes queued")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(online: bool, pending_ops: usize) -> NetState {
        NetState {
            online,
            pending_ops,
            dropped_ops: 0,
            stuck_ops: 0,
        }
    }

    #[test]
    fn pill_label_hides_when_online_and_idle() {
        assert_eq!(pill_label(&state(true, 0)), None);
    }

    #[test]
    fn pill_label_reports_offline_queue_depth() {
        assert_eq!(pill_label(&state(false, 0)).as_deref(), Some("Offline"));
        assert_eq!(
            pill_label(&state(false, 1)).as_deref(),
            Some("Offline \u{00b7} 1 change queued")
        );
        assert_eq!(
            pill_label(&state(false, 3)).as_deref(),
            Some("Offline \u{00b7} 3 changes queued")
        );
    }

    #[test]
    fn pill_label_shows_syncing_while_draining_after_reconnect() {
        assert_eq!(
            pill_label(&state(true, 2)).as_deref(),
            Some("Syncing \u{00b7} 2 pending")
        );
    }
}
