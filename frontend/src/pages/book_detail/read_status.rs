//! Read/unread control for the book-detail hero — mark a book unread / reading /
//! finished without a journal entry. A three-segment control whose first paint
//! matches SSR (`Unread`, "Not started"); a post-mount effect loads saved state
//! and reconciles (rule 07), and clicks write optimistically with an op-sequence
//! guard, reverting on error — mirroring the rating widget.

use dioxus::prelude::*;
use omnibus_shared::{ReadStatus, ReadStatusRecord, SetReadStatus};

use crate::time::now_unix;
use crate::{data, use_server_url};

/// The three selectable states, in display order, with their labels + testids.
const SEGMENTS: [(ReadStatus, &str, &str); 3] = [
    (ReadStatus::Unread, "Unread", "read-status-unread"),
    (ReadStatus::Reading, "Reading", "read-status-reading"),
    (ReadStatus::Finished, "Finished", "read-status-finished"),
];

/// Segmented read-state control bound to the current user and `uuid`.
#[component]
pub(super) fn BdReadStatusControl(uuid: String) -> Element {
    let server_url = use_server_url();
    // `current` is the persisted record; both SSR and first-hydration paint
    // render `None` (→ Unread), and the effect below reconciles on the client.
    let mut current = use_signal(|| None::<ReadStatusRecord>);
    let mut failed = use_signal(|| false);
    // Monotonic write counter: a save only applies its result if it's still the
    // latest, so a slow (out-of-order) response can't clobber a newer click.
    let mut op_seq = use_signal(|| 0u64);
    let mut load_seq = use_signal(|| 0u64);

    let load_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        if uuid.is_empty() {
            return;
        }
        let my_load = *load_seq.peek() + 1;
        let next_op = *op_seq.peek() + 1;
        load_seq.set(my_load);
        op_seq.set(next_op);
        current.set(None);
        failed.set(false);
        let load_url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Ok(rec) = data::get_read_status(&load_url, &uuid).await {
                if *load_seq.peek() == my_load {
                    current.set(rec);
                }
            }
        });
    }));

    let shown = current().map(|r| r.status).unwrap_or_default();
    let meta_text = status_meta(now_unix(), failed(), current().as_ref());

    rsx! {
        div {
            class: "bd-readstatus",
            "data-testid": "read-status-control",
            div {
                class: "bd-readstatus-seg",
                role: "group",
                aria_label: "Reading status",
                for (status, label, testid) in SEGMENTS {
                    {
                        let active = shown == status;
                        let uuid = uuid.clone();
                        let server_url = server_url.clone();
                        rsx! {
                            button {
                                key: "{testid}",
                                r#type: "button",
                                class: if active { "bd-readstatus-btn active" } else { "bd-readstatus-btn" },
                                "data-testid": testid,
                                aria_pressed: active,
                                onclick: move |_| {
                                    if uuid.is_empty() || active {
                                        return;
                                    }
                                    apply_status(
                                        current, failed, op_seq, uuid.clone(), server_url.clone(), status,
                                    );
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
            div {
                class: "mono bd-readstatus-meta",
                role: "status",
                "data-testid": "read-status-meta",
                "{meta_text}"
            }
        }
    }
}

/// Optimistically set `current`, then persist and reconcile with the server
/// (reverting and flagging on error). `op_seq` is bumped per call and
/// re-checked after the request so a stale (out-of-order) response is dropped.
fn apply_status(
    mut current: Signal<Option<ReadStatusRecord>>,
    mut failed: Signal<bool>,
    mut op_seq: Signal<u64>,
    uuid: String,
    server_url: String,
    status: ReadStatus,
) {
    let prev = current();
    let my_op = op_seq() + 1;
    op_seq.set(my_op);
    failed.set(false);
    current.set(Some(ReadStatusRecord {
        book_uuid: uuid.clone(),
        status,
        updated_at: now_unix(),
        finished_at: (status == ReadStatus::Finished).then(now_unix),
    }));
    spawn(async move {
        let result = data::set_read_status(
            &server_url,
            SetReadStatus {
                book_uuid: uuid,
                status,
            },
        )
        .await;
        // A newer click superseded this one — drop the stale result.
        if op_seq() != my_op {
            return;
        }
        match result {
            Ok(rec) => current.set(Some(rec)),
            Err(_) => {
                current.set(prev);
                failed.set(true);
            }
        }
    });
}

/// Status line under the control: an error nudge, a relative "finished N ago"
/// for completed books, or the plain current state.
fn status_meta(now: i64, failed: bool, current: Option<&ReadStatusRecord>) -> String {
    if failed {
        return "Couldn't save — try again".to_string();
    }
    match current.map(|r| r.status).unwrap_or_default() {
        ReadStatus::Unread => "Not started".to_string(),
        ReadStatus::Reading => "In progress".to_string(),
        ReadStatus::Finished => match current.and_then(|r| r.finished_at) {
            Some(at) => format!("Finished {}", ago(now, at)),
            None => "Finished".to_string(),
        },
    }
}

/// Relative "N ago" phrase from a unix-seconds timestamp, computed against the
/// injected `now` so it stays clock-injectable and unit-testable.
fn ago(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    let plural = |n: i64| if n == 1 { "" } else { "s" };
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        let m = secs / 60;
        format!("{m} minute{} ago", plural(m))
    } else if secs < 86_400 {
        let h = secs / 3600;
        format!("{h} hour{} ago", plural(h))
    } else {
        let d = secs / 86_400;
        format!("{d} day{} ago", plural(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(status: ReadStatus, finished_at: Option<i64>) -> ReadStatusRecord {
        ReadStatusRecord {
            book_uuid: "b".into(),
            status,
            updated_at: 0,
            finished_at,
        }
    }

    #[test]
    fn status_meta_reports_not_started_when_absent() {
        assert_eq!(status_meta(1_000, false, None), "Not started");
    }

    #[test]
    fn status_meta_reports_in_progress_for_reading() {
        assert_eq!(
            status_meta(1_000, false, Some(&rec(ReadStatus::Reading, None))),
            "In progress"
        );
    }

    #[test]
    fn status_meta_reports_relative_finish_time() {
        let r = rec(ReadStatus::Finished, Some(1_000));
        assert_eq!(
            status_meta(1_000 + 3 * 3600, false, Some(&r)),
            "Finished 3 hours ago"
        );
    }

    #[test]
    fn status_meta_finished_without_timestamp_falls_back() {
        assert_eq!(
            status_meta(1_000, false, Some(&rec(ReadStatus::Finished, None))),
            "Finished"
        );
    }

    #[test]
    fn status_meta_surfaces_failure() {
        assert_eq!(
            status_meta(1_000, true, Some(&rec(ReadStatus::Reading, None))),
            "Couldn't save — try again"
        );
    }

    #[test]
    fn ago_singularizes_one_day() {
        assert_eq!(ago(172_800, 86_400), "1 day ago");
    }
}

#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;

    /// The first (SSR / pre-load) paint renders the three segments and the
    /// unread default, so hydration adopts identical markup (rule 07).
    #[test]
    fn control_renders_all_three_segments_unread_by_default() {
        let html = dioxus::ssr::render_element(rsx! {
            BdReadStatusControl { uuid: "book-uuid".to_string() }
        });
        assert!(html.contains("data-testid=\"read-status-control\""));
        assert!(html.contains("data-testid=\"read-status-unread\""));
        assert!(html.contains("data-testid=\"read-status-reading\""));
        assert!(html.contains("data-testid=\"read-status-finished\""));
        assert!(html.contains("Not started"));
        // The unread segment is the active one at first paint.
        assert!(
            html.contains("bd-readstatus-btn active"),
            "expected an active segment, html was {html}"
        );
    }
}
