//! "Downloads" section of the mobile "You" tab: every book stored for
//! offline use, with per-row Remove and a total-storage line. Live-updates
//! off the download registry's watch channel.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::offline::downloads::{self, DlFormat, DownloadEntry, DownloadStatus};
use crate::offline::{store, sync};
use crate::Route;

/// The Downloads card. Hidden entirely while nothing is downloaded (or
/// downloading) — the You tab stays quiet until the feature is used.
#[component]
pub(super) fn DownloadsSection() -> Element {
    let mut generation = use_signal(|| 0u64);
    use_future(move || async move {
        let mut rx = downloads::subscribe();
        loop {
            let now = *rx.borrow_and_update();
            if now != generation() {
                generation.set(now);
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
    let _generation = generation();
    let entries = downloads::list();
    if entries.is_empty() {
        return rsx! {};
    }
    let total = downloads::total_complete_bytes();

    rsx! {
        div { class: "m-account-downloads", "data-testid": "account-downloads",
            div { class: "m-section-head",
                div { class: "label", "Downloads" }
                div { class: "m-account-dl-total mono", "{downloads::format_bytes(total)} on device" }
            }
            div { class: "m-account-rows",
                for entry in entries {
                    DownloadRow { key: "{entry.book_uuid}:{entry.format.as_str()}", entry: entry.clone() }
                }
            }
        }
    }
}

/// Sync status line for the You tab: connectivity, queued-change count,
/// permanently-rejected count, and the last successful drain time.
#[component]
pub(super) fn SyncStatusRow() -> Element {
    let mut net = use_signal(sync::state);
    let mut last_sync = use_signal(|| None::<i64>);
    use_future(move || async move {
        let mut rx = sync::subscribe();
        loop {
            let now = *rx.borrow_and_update();
            if now != net() {
                net.set(now);
            }
            let stamp = match store::store() {
                Some(st) => st
                    .meta_get("last_sync_at")
                    .await
                    .and_then(|s| s.parse::<i64>().ok()),
                None => None,
            };
            if stamp != last_sync() {
                last_sync.set(stamp);
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });

    let state = net();
    let headline = match (state.online, state.pending_ops) {
        (true, 0) => "Synced".to_string(),
        (true, n) => format!("Syncing \u{2014} {n} pending"),
        (false, 0) => "Offline".to_string(),
        (false, n) => format!("Offline \u{2014} {n} changes waiting to sync"),
    };
    rsx! {
        div { class: "m-account-sync", "data-testid": "account-sync-status",
            div { class: "m-account-sync-line",
                span {
                    class: if state.online { "m-account-sync-dot on" } else { "m-account-sync-dot" },
                    "aria-hidden": "true",
                }
                "{headline}"
            }
            if state.dropped_ops > 0 {
                div { class: "m-account-sync-dropped",
                    "{state.dropped_ops} changes couldn't sync and were discarded."
                }
            }
            if state.stuck_ops > 0 {
                div { class: "m-account-sync-dropped", "data-testid": "account-sync-stuck",
                    "{state.stuck_ops} changes failed repeatedly and were dropped after retrying."
                }
            }
            if let Some(at) = last_sync() {
                div { class: "m-account-sync-when mono", "Last synced {relative_time(at)}" }
            }
        }
    }
}

/// Coarse "how long ago" for the last-synced line.
fn relative_time(at: i64) -> String {
    let delta = (store::now_secs() - at).max(0);
    match delta {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", delta / 60),
        3600..=86399 => format!("{}h ago", delta / 3600),
        _ => format!("{}d ago", delta / 86400),
    }
}

/// The row's secondary line: what this format is, and what state it's in.
fn subline(format_label: &str, status: &DownloadStatus, stale: bool) -> String {
    match status {
        DownloadStatus::Complete { bytes } if stale => format!(
            "{format_label} \u{00b7} Update available \u{00b7} {}",
            downloads::format_bytes(*bytes)
        ),
        DownloadStatus::Complete { bytes } => format!(
            "{format_label} \u{00b7} {}",
            downloads::format_bytes(*bytes)
        ),
        DownloadStatus::Downloading { downloaded, .. } => format!(
            "{format_label} \u{00b7} downloading \u{2026} {}",
            downloads::format_bytes(*downloaded)
        ),
        DownloadStatus::Error { message } => format!("{format_label} \u{00b7} {message}"),
        DownloadStatus::NotDownloaded => format_label.to_string(),
    }
}

/// One downloaded (or in-flight) book row: title + format/size subline,
/// with Remove for finished rows — or Re-download when the library file has
/// moved under this copy.
#[component]
fn DownloadRow(entry: DownloadEntry) -> Element {
    let format_label = match entry.format {
        DlFormat::Epub => "Ebook",
        DlFormat::Audio => "Audiobook",
    };
    // A completed row whose library file has since moved reads as an
    // update, not a size — the number is still true but it is no longer
    // the interesting fact about this row.
    let stale = entry.stale && matches!(entry.status, DownloadStatus::Complete { .. });
    let subline = subline(format_label, &entry.status, stale);
    let title = if entry.title.is_empty() {
        "Untitled".to_string()
    } else {
        entry.title.clone()
    };
    let removable = matches!(
        entry.status,
        DownloadStatus::Complete { .. } | DownloadStatus::Error { .. }
    );
    let (uuid, format) = (entry.book_uuid.clone(), entry.format);
    let on_remove = {
        let uuid = uuid.clone();
        move |_| downloads::remove(&uuid, format)
    };
    // Same single operation as the book-detail row — see `redownload` for
    // why this is not `remove` followed by `start`.
    let on_redownload = {
        let (uuid, title, server_url) = (uuid, title.clone(), crate::use_server_url());
        move |_| {
            downloads::redownload(
                server_url.clone(),
                uuid.clone(),
                format,
                entry.file_id,
                title.clone(),
            );
        }
    };
    let sub_class = if stale {
        "m-account-dl-sub m-dl-row-stale"
    } else {
        "m-account-dl-sub"
    };

    rsx! {
        div { class: "m-account-row m-account-dl-row", "data-testid": "account-download-row",
            Link {
                to: Route::BookDetail { uuid: entry.book_uuid.clone() },
                class: "m-account-dl-link",
                div { class: "m-account-dl-title", "{title}" }
                div { class: "{sub_class}", "{subline}" }
            }
            if stale {
                button {
                    r#type: "button",
                    class: "btn sm",
                    "data-testid": "account-download-redownload",
                    "aria-label": "Re-download {title} to update this device's copy",
                    onclick: on_redownload,
                    "Re-download"
                }
            } else if removable {
                button {
                    r#type: "button",
                    class: "btn sm ghost m-dl-remove",
                    "aria-label": "Remove {title} from this device",
                    onclick: on_remove,
                    "Remove"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subline_leads_with_the_update_when_the_library_file_moved() {
        let status = DownloadStatus::Complete { bytes: 2048 };
        assert_eq!(
            subline("Ebook", &status, true),
            "Ebook \u{00b7} Update available \u{00b7} 2.0 KB"
        );
        assert_eq!(subline("Ebook", &status, false), "Ebook \u{00b7} 2.0 KB");
    }

    #[test]
    fn subline_ignores_staleness_for_a_row_that_is_not_complete() {
        // Nothing finished downloading, so there is no local copy for a
        // newer file to be newer *than*.
        let downloading = DownloadStatus::Downloading {
            downloaded: 1024,
            total: Some(4096),
        };
        assert_eq!(
            subline("Audiobook", &downloading, true),
            "Audiobook \u{00b7} downloading \u{2026} 1.0 KB"
        );
        let failed = DownloadStatus::Error {
            message: "Connection lost".into(),
        };
        assert_eq!(
            subline("Audiobook", &failed, true),
            "Audiobook \u{00b7} Connection lost"
        );
    }
}
