//! Mobile offline-download section for the book-detail page: one row per
//! downloadable format (ebook / audiobook) walking the Download →
//! progress → Downloaded/Remove state machine against the
//! [`crate::offline::downloads`] registry.

use dioxus::prelude::*;

use crate::offline::downloads::{self, DlFormat, DownloadStatus};
use crate::use_server_url;

/// The "Available offline" section. Rendered only on books with at least
/// one downloadable format and only for users whose `can_download` bit is
/// set (the default).
#[component]
pub(super) fn BdOfflineSection(
    uuid: String,
    title: String,
    has_ebook: bool,
    has_audio: bool,
    stale_ebook: bool,
    stale_audio: bool,
) -> Element {
    // Re-render on every registry transition (progress %, completion,
    // removal) — the registry generation rides a watch channel per the
    // token_store subscriber pattern.
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
    let user = crate::use_current_user_summary();

    if !has_ebook && !has_audio {
        return rsx! {};
    }
    // Hidden (not disabled) when the account can't download — matches how
    // other permission-gated affordances behave on mobile.
    if !user().map(|u| u.can_download).unwrap_or(true) {
        return rsx! {};
    }
    // Read the generation so registry bumps re-render this section. The
    // per-row `status` is computed HERE and passed down as a prop: `DlRow`'s
    // other props never change, so without a changing prop Dioxus would
    // memoize the row and skip re-rendering it — the Download/Remove tap
    // would appear to do nothing until the page remounted.
    let _generation = generation();

    rsx! {
        section { class: "m-section", "data-testid": "mobile-offline-section",
            div { class: "label", "Available offline" }
            div { class: "m-dl-rows",
                if has_ebook {
                    DlRow {
                        uuid: uuid.clone(),
                        title: title.clone(),
                        format_label: "Ebook",
                        format: DlFormat::Epub,
                        status: downloads::status(&uuid, DlFormat::Epub),
                        stale: stale_ebook,
                    }
                }
                if has_audio {
                    DlRow {
                        uuid: uuid.clone(),
                        title,
                        format_label: "Audiobook",
                        format: DlFormat::Audio,
                        status: downloads::status(&uuid, DlFormat::Audio),
                        stale: stale_audio,
                    }
                }
            }
        }
    }
}

/// One per-format row: status line on the left, the state's action on the
/// right. `status` arrives as a prop (not read from the registry here) so
/// the row re-renders on every registry transition — see the memoization
/// note in [`BdOfflineSection`].
#[component]
fn DlRow(
    uuid: String,
    title: String,
    format_label: &'static str,
    format: DlFormat,
    status: DownloadStatus,
    stale: bool,
) -> Element {
    let server_url = use_server_url();

    let start = {
        let (uuid, title, server_url) = (uuid.clone(), title.clone(), server_url.clone());
        move |_| {
            downloads::start(
                server_url.clone(),
                uuid.clone(),
                format,
                None,
                title.clone(),
            );
        }
    };
    let remove = {
        let uuid = uuid.clone();
        move |_| downloads::remove(&uuid, format)
    };
    // Remove first: `start` refuses to run against a Complete entry, and the
    // bytes on disk are the superseded ones either way.
    let redownload = {
        let (uuid, title, server_url) = (uuid.clone(), title.clone(), server_url.clone());
        move |_| {
            downloads::remove(&uuid, format);
            downloads::start(
                server_url.clone(),
                uuid.clone(),
                format,
                None,
                title.clone(),
            );
        }
    };

    let testid = format!("offline-row-{}", format.as_str());
    rsx! {
        div { class: "m-dl-row", "data-testid": "{testid}",
            div { class: "m-dl-row-body",
                div { class: "m-dl-row-label", "{format_label}" }
                match &status {
                    DownloadStatus::NotDownloaded => rsx! {
                        div { class: "m-dl-row-sub", "Not on this device" }
                    },
                    DownloadStatus::Downloading { downloaded, total } => rsx! {
                        div { class: "m-dl-row-sub", "{progress_label(*downloaded, *total)}" }
                        div { class: "m-dl-bar",
                            i { style: "width: {progress_pct(*downloaded, *total)}%;" }
                        }
                    },
                    DownloadStatus::Error { message } => rsx! {
                        div { class: "m-dl-row-sub m-dl-row-error", "{message}" }
                    },
                    DownloadStatus::Complete { bytes } if stale => rsx! {
                        div { class: "m-dl-row-sub m-dl-row-stale",
                            "Update available \u{00b7} {downloads::format_bytes(*bytes)} on this device"
                        }
                    },
                    DownloadStatus::Complete { bytes } => rsx! {
                        div { class: "m-dl-row-sub m-dl-row-ok",
                            {downloaded_check()}
                            "Downloaded \u{00b7} {downloads::format_bytes(*bytes)}"
                        }
                    },
                }
            }
            match &status {
                DownloadStatus::NotDownloaded => rsx! {
                    button { r#type: "button", class: "btn sm", onclick: start, "Download" }
                },
                DownloadStatus::Downloading { .. } => rsx! {
                    span { class: "m-dl-row-busy mono", "\u{2026}" }
                },
                DownloadStatus::Error { .. } => rsx! {
                    button { r#type: "button", class: "btn sm", onclick: start, "Retry" }
                },
                DownloadStatus::Complete { .. } if stale => rsx! {
                    button {
                        r#type: "button",
                        class: "btn sm",
                        "data-testid": "offline-redownload-{format.as_str()}",
                        onclick: redownload,
                        "Re-download"
                    }
                },
                DownloadStatus::Complete { .. } => rsx! {
                    button {
                        r#type: "button",
                        class: "btn sm ghost m-dl-remove",
                        "data-testid": "offline-remove-{format.as_str()}",
                        onclick: remove,
                        "Remove"
                    }
                },
            }
        }
    }
}

/// "12.3 MB of 234 MB" (or a bare running total when no estimate exists).
fn progress_label(downloaded: i64, total: Option<i64>) -> String {
    match total {
        Some(total) if total > 0 => format!(
            "{} of {}",
            downloads::format_bytes(downloaded),
            downloads::format_bytes(total)
        ),
        _ => format!("{} downloaded", downloads::format_bytes(downloaded)),
    }
}

/// Clamped 0–100 progress width; an unknown total shows a sliver so the
/// bar reads as active rather than stalled.
fn progress_pct(downloaded: i64, total: Option<i64>) -> i64 {
    match total {
        Some(total) if total > 0 => ((downloaded * 100) / total).clamp(0, 100),
        _ => 6,
    }
}

fn downloaded_check() -> Element {
    rsx! {
        svg {
            class: "m-dl-check",
            width: "12", height: "12", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "3", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M20 6L9 17l-5-5" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{progress_label, progress_pct};

    #[test]
    fn progress_label_reports_of_total_when_estimated() {
        assert_eq!(progress_label(1024, Some(2048)), "1.0 KB of 2.0 KB");
        assert_eq!(progress_label(1024, None), "1.0 KB downloaded");
        assert_eq!(progress_label(512, Some(0)), "512 B downloaded");
    }

    #[test]
    fn progress_pct_clamps_and_falls_back_without_total() {
        assert_eq!(progress_pct(50, Some(200)), 25);
        assert_eq!(progress_pct(300, Some(200)), 100);
        assert_eq!(progress_pct(50, None), 6);
    }
}
