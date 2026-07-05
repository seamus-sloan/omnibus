//! Per-format CTA rows on the book detail page. Renders one row per format
//! the book has, sorted alphabetically, with per-format actions wired
//! underneath. When multiple files of the same format exist (after merge),
//! renders sub-rows with a file picker so the user can choose which file
//! to play or read.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::Link;

use omnibus_shared::BookFileInfo;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::KindleSendStatus;

#[cfg(not(feature = "mobile"))]
use crate::Route;

/// Renders the format switcher, letting the reader pick which available
/// format of a book to open.
#[component]
pub fn FormatSwitcher(
    formats: Vec<String>,
    uuid: String,
    #[props(default)] book_files: Vec<BookFileInfo>,
) -> Element {
    let rows = prepare_rows(&formats);
    if rows.is_empty() {
        return rsx! {};
    }

    rsx! {
        div {
            class: "format-switcher",
            role: "group",
            aria_label: "Available formats",
            "data-testid": "format-switcher",
            for row in rows {
                {
                    let files_for_format: Vec<&BookFileInfo> = book_files.iter()
                        .filter(|f| FormatKind::from_raw(&f.format) == row)
                        .collect();
                    if files_for_format.len() > 1 {
                        rsx! {
                            MultiFileRow {
                                key: "{row.label()}",
                                kind: row,
                                uuid: uuid.clone(),
                                files: files_for_format.into_iter().cloned().collect(),
                            }
                        }
                    } else {
                        rsx! {
                            FormatRow { key: "{row.label()}", kind: row, uuid: uuid.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FormatRow(kind: FormatKind, uuid: String) -> Element {
    let label = kind.label();
    let testid = format!("format-row-{}", label.to_ascii_lowercase());
    rsx! {
        div {
            class: "format-row",
            "data-format": "{label}",
            "data-testid": "{testid}",
            span { class: "format-badge", "data-testid": "format-badge", "{label}" }
            div { class: "format-actions",
                match kind {
                    FormatKind::Epub => rsx! {
                        // F2.2: web routes into the immersive reader; mobile
                        // stays disabled (no JS engine for epub.js — see F6.2).
                        // The cfg lives at the helper definition (rule 07:
                        // hydration parity — keep cfg gates out of rsx).
                        {read_book_action(&uuid)}
                        {send_to_kindle_action(&uuid, None)}
                        {send_to_kobo_action(&uuid)}
                    },
                    FormatKind::M4b | FormatKind::Mp3 => rsx! {
                        // F2.3: web routes into the immersive player; mobile
                        // stays disabled (no `<audio>` binding in the Dioxus
                        // Native shell yet — see F6.x).
                        {listen_book_action(&uuid)}
                    },
                    FormatKind::Other(_) => rsx! {
                        span { class: "format-actions-empty", "No actions yet" }
                    },
                }
            }
        }
    }
}

/// Expandable row for formats with multiple files (after merge). Shows the
/// format badge with a file count, and sub-rows for each file.
#[component]
fn MultiFileRow(kind: FormatKind, uuid: String, files: Vec<BookFileInfo>) -> Element {
    let label = kind.label();
    let testid = format!("format-row-{}", label.to_ascii_lowercase());
    let count = files.len();

    rsx! {
        div {
            class: "format-row format-row-multi",
            "data-format": "{label}",
            "data-testid": "{testid}",
            span { class: "format-badge", "data-testid": "format-badge",
                "{label} ({count} files)"
            }
            // Per-file Read/Listen live in the sub-rows below, but "Send to
            // Kobo" is book-level: the KEPUB endpoint converts the primary
            // (lowest-ordinal) EPUB, so a multi-EPUB book still gets the CTA
            // here. Per-file KEPUB is a deferred follow-up.
            if kind == FormatKind::Epub {
                div { class: "format-actions", {send_to_kobo_action(&uuid)} }
            }
        }
        for file in &files {
            {
                let file_label = file.label.clone()
                    .unwrap_or_else(|| format!("Part {}", file.ordinal + 1));
                let file_testid = format!("format-file-{}", file.id);
                let file_id = file.id;
                rsx! {
                    div {
                        key: "{file_id}",
                        class: "format-row format-subrow",
                        "data-testid": "{file_testid}",
                        span { class: "format-sublabel", "{file_label}" }
                        div { class: "format-actions",
                            match kind {
                                // Per-file actions delegate to platform-gated
                                // helpers (rule 07: hydration parity — keep
                                // cfg gates out of rsx bodies).
                                FormatKind::Epub => rsx! {
                                    {read_file_action(&uuid, file_id)}
                                    {send_to_kindle_action(&uuid, Some(file_id))}
                                },
                                FormatKind::M4b | FormatKind::Mp3 => rsx! {
                                    {listen_file_action(&uuid, file_id)}
                                },
                                FormatKind::Other(_) => rsx! {
                                    span { class: "format-actions-empty", "No actions yet" }
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Per-target action helpers ────────────────────────────────────
//
// The cfg gate lives at the helper definition, not inside an rsx body.
// SSR (`feature = "server"`) and WASM (`feature = "web"`) both hit the
// `not(feature = "mobile")` arm and emit an identical `<a>` Link — so
// hydration parity holds (rule 07). Mobile (`feature = "mobile"`) is
// Dioxus Native, doesn't hydrate, and renders a disabled `<button>` as
// a placeholder until F6.x lands the native reader/player.

/// "Read" CTA for the book-level row. Web/SSR routes into the immersive
/// reader; mobile renders a disabled placeholder.
#[cfg(not(feature = "mobile"))]
fn read_book_action(uuid: &str) -> Element {
    rsx! {
        Link {
            to: Route::BookRead { uuid: uuid.to_string() },
            class: "btn",
            "data-testid": "action-read",
            "Read"
        }
    }
}

#[cfg(feature = "mobile")]
fn read_book_action(_uuid: &str) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Reading on mobile coming soon",
            "data-testid": "action-read",
            "Read"
        }
    }
}

/// F4.1 "Send to Kobo" CTA: downloads the book as KEPUB (`GET
/// /api/ebooks/:uuid/kepub`) for USB sideload onto a Kobo. Web/SSR renders a
/// plain download `<a>` — a full-navigation download carries the session
/// cookie, so it authenticates like the reader's file fetch. Mobile renders a
/// disabled placeholder (the copy-over-USB flow is desktop-only).
#[cfg(not(feature = "mobile"))]
fn send_to_kobo_action(uuid: &str) -> Element {
    let href = format!("/api/ebooks/{uuid}/kepub");
    rsx! {
        a {
            href: "{href}",
            download: "",
            class: "btn",
            title: "Download as KEPUB to copy onto a Kobo over USB",
            "data-testid": "action-kobo",
            "Send to Kobo"
        }
    }
}

#[cfg(feature = "mobile")]
fn send_to_kobo_action(_uuid: &str) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Send-to-Kobo coming soon",
            "data-testid": "action-kobo",
            "Send to Kobo"
        }
    }
}

/// "Send to Kindle" CTA (F4.3). Web/SSR renders the interactive
/// [`SendToKindleButton`]; mobile renders a disabled placeholder. The cfg gate
/// lives at the helper definition (rule 07: keep cfg out of rsx bodies), and
/// SSR + first WASM paint emit the same enabled button so hydration holds.
#[cfg(not(feature = "mobile"))]
fn send_to_kindle_action(uuid: &str, file_id: Option<i64>) -> Element {
    rsx! {
        SendToKindleButton { uuid: uuid.to_string(), file_id }
    }
}

#[cfg(feature = "mobile")]
fn send_to_kindle_action(_uuid: &str, _file_id: Option<i64>) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Send-to-Kindle on mobile coming soon",
            "data-testid": "action-kindle",
            "Send to Kindle"
        }
    }
}

/// Interactive Send-to-Kindle button. On click it enqueues the job (fast, so
/// it never trips the server's 30s request-timeout guard) and then polls the
/// worker for the delivery outcome, showing "Sending…" in-place meanwhile. On a
/// terminal state it raises a bottom-center toast (matching the merge/bookmark
/// toasts): a success toast auto-dismisses after a few seconds, an error toast
/// stays until dismissed so the message stays readable. Disabled while in
/// flight. `class` / `testid` default to the per-format-row styling; the hero
/// CTA overrides them to render a large ghost button with its own testid.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn SendToKindleButton(
    uuid: String,
    file_id: Option<i64>,
    #[props(default = "btn".to_string())] class: String,
    #[props(default = "action-kindle".to_string())] testid: String,
) -> Element {
    let server_url = crate::use_server_url();
    let mut in_flight = use_signal(|| false);
    // (is_error, message) — None until the first send completes / toast dismissed.
    let mut result = use_signal(|| None::<(bool, String)>);

    rsx! {
        button {
            class: "{class}",
            disabled: in_flight(),
            "data-testid": "{testid}",
            onclick: move |_| {
                let url = server_url.clone();
                let uuid = uuid.clone();
                in_flight.set(true);
                result.set(None);
                spawn(async move {
                    // Enqueue; a fast pre-check failure (no Kindle email, SMTP
                    // unconfigured, unknown book) comes back here immediately.
                    let task_id = match crate::data::enqueue_send_to_kindle(&url, &uuid, file_id).await {
                        Ok(id) => id,
                        Err(e) => {
                            result.set(Some((true, format!("Send failed: {e}"))));
                            in_flight.set(false);
                            return;
                        }
                    };
                    let (is_error, message) = poll_send_result(&url, task_id).await;
                    result.set(Some((is_error, message)));
                    in_flight.set(false);
                    // Success is transient — auto-dismiss the toast. Errors stay
                    // until the user dismisses them.
                    if !is_error {
                        async_sleep_ms(4000).await;
                        result.set(None);
                    }
                });
            },
            if in_flight() { "Sending\u{2026}" } else { "Send to Kindle" }
        }
        if let Some((is_error, message)) = result() {
            div { class: "kindle-toast card", role: "status",
                span {
                    "data-testid": "kindle-send-status",
                    class: if is_error { "kindle-toast-msg error" } else { "kindle-toast-msg success" },
                    "{message}"
                }
                button {
                    class: "btn ghost sm",
                    "data-testid": "kindle-toast-dismiss",
                    aria_label: "Dismiss",
                    onclick: move |_| result.set(None),
                    "\u{00d7}"
                }
            }
        }
    }
}

/// Poll the worker until the enqueued send reaches a terminal state, mapping it
/// to the toast's `(is_error, message)` pair. `Ok(None)` means the task id went
/// unknown before we saw a terminal state (evicted past the worker's retention
/// window) — rare under sub-second polling, surfaced as a soft error since we
/// can't confirm delivery.
#[cfg(not(feature = "mobile"))]
async fn poll_send_result(url: &str, task_id: u64) -> (bool, String) {
    const POLL_INTERVAL_MS: u32 = 700;
    loop {
        async_sleep_ms(POLL_INTERVAL_MS).await;
        match crate::data::kindle_send_status(url, task_id).await {
            Ok(Some(KindleSendStatus::Pending)) => continue,
            Ok(Some(KindleSendStatus::Sent)) => return (false, "Sent to your Kindle.".to_string()),
            Ok(Some(KindleSendStatus::Failed { message })) => {
                return (true, format!("Send failed: {message}"))
            }
            Ok(None) => {
                return (
                    true,
                    "Send failed: could not confirm the send completed.".to_string(),
                )
            }
            Err(e) => return (true, format!("Send failed: {e}")),
        }
    }
}

// ── Platform-gated poll sleeper ──────────────────────────────────
//
// Mirrors `worker_status`'s helper: web uses `gloo_timers`; the SSR/server
// build (where the click handler never actually runs — it only needs to
// compile) falls back to `tokio::time::sleep`.

#[cfg(all(not(feature = "mobile"), feature = "web"))]
async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(all(not(feature = "mobile"), not(feature = "web"), feature = "server"))]
async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

/// "Listen" CTA for the book-level row.
#[cfg(not(feature = "mobile"))]
fn listen_book_action(uuid: &str) -> Element {
    rsx! {
        Link {
            to: Route::BookListen { uuid: uuid.to_string() },
            class: "btn",
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

#[cfg(feature = "mobile")]
fn listen_book_action(_uuid: &str) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Listening on mobile coming soon",
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

/// Per-file "Read" CTA used inside a `MultiFileRow`. Routes carry a
/// `file_id` query so the reader opens the chosen file.
#[cfg(not(feature = "mobile"))]
fn read_file_action(uuid: &str, file_id: i64) -> Element {
    let href = format!("/read/{uuid}?file_id={file_id}");
    rsx! {
        Link {
            to: "{href}",
            class: "btn",
            "data-testid": "action-read",
            "Read"
        }
    }
}

#[cfg(feature = "mobile")]
fn read_file_action(_uuid: &str, _file_id: i64) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            "data-testid": "action-read",
            "Read"
        }
    }
}

/// Per-file "Listen" CTA used inside a `MultiFileRow`.
#[cfg(not(feature = "mobile"))]
fn listen_file_action(uuid: &str, file_id: i64) -> Element {
    let href = format!("/listen/{uuid}?file_id={file_id}");
    rsx! {
        Link {
            to: "{href}",
            class: "btn",
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

#[cfg(feature = "mobile")]
fn listen_file_action(_uuid: &str, _file_id: i64) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

/// One row in the switcher. `Other(String)` keeps the original casing of the
/// raw `book_files.format` value so the badge displays whatever the schema
/// stored (e.g. "PDF", "CBZ") without invoking a giant match.
#[derive(Clone, PartialEq, Eq)]
enum FormatKind {
    Epub,
    M4b,
    Mp3,
    Other(String),
}

impl FormatKind {
    fn from_raw(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("EPUB") {
            FormatKind::Epub
        } else if raw.eq_ignore_ascii_case("M4B") || raw.eq_ignore_ascii_case("M4A") {
            // M4A is the same MPEG-4 container as M4B with a different
            // extension; both flow through the F2.3 player and the
            // tower-http serve handler under the same code path.
            FormatKind::M4b
        } else if raw.eq_ignore_ascii_case("MP3") {
            FormatKind::Mp3
        } else {
            FormatKind::Other(raw.to_string())
        }
    }

    fn label(&self) -> &str {
        match self {
            FormatKind::Epub => "EPUB",
            FormatKind::M4b => "M4B",
            FormatKind::Mp3 => "MP3",
            FormatKind::Other(s) => s.as_str(),
        }
    }
}

/// Dedupe (case-insensitive), sort alphabetical by label (also case-
/// insensitive — otherwise unknown-cased rows like `"cbz"` would sort after
/// the upper-cased known ones, which doesn't match the "alphabetical"
/// contract or the dedupe logic), and map raw format strings to the typed
/// rows the switcher renders.
fn prepare_rows(formats: &[String]) -> Vec<FormatKind> {
    let mut rows: Vec<FormatKind> = formats.iter().map(|s| FormatKind::from_raw(s)).collect();
    rows.sort_by(|a, b| {
        a.label()
            .to_ascii_lowercase()
            .cmp(&b.label().to_ascii_lowercase())
    });
    rows.dedup_by(|a, b| a.label().eq_ignore_ascii_case(b.label()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_rows_sorts_alphabetical() {
        let rows = prepare_rows(&["M4B".into(), "EPUB".into(), "PDF".into()]);
        assert_eq!(
            rows.iter()
                .map(super::FormatKind::label)
                .collect::<Vec<_>>(),
            vec!["EPUB", "M4B", "PDF"],
        );
    }

    #[test]
    fn prepare_rows_dedupes_case_insensitively() {
        let rows = prepare_rows(&["epub".into(), "EPUB".into()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label(), "EPUB");
    }

    #[test]
    fn prepare_rows_sorts_case_insensitively() {
        // Regression for PR #65 review: mixed-case input must produce a
        // consistent alphabetical order regardless of casing — otherwise
        // upper-cased known formats (EPUB, M4B) would always sort before
        // lower-cased unknown ones (cbz), which surprises users.
        let rows = prepare_rows(&["PDF".into(), "cbz".into(), "EPUB".into()]);
        assert_eq!(
            rows.iter()
                .map(super::FormatKind::label)
                .collect::<Vec<_>>(),
            vec!["cbz", "EPUB", "PDF"],
        );
    }

    #[test]
    fn prepare_rows_preserves_unknown_format_casing() {
        let rows = prepare_rows(&["CbZ".into()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label(), "CbZ");
        assert!(matches!(rows[0], FormatKind::Other(_)));
    }

    #[test]
    fn empty_input_renders_nothing_meaningful() {
        // We don't exercise the actual rsx! macro (no SSR dep in this crate),
        // but the prepare_rows path is what gates the FormatSwitcher's
        // `rows.is_empty() → return rsx!{}` branch.
        assert!(prepare_rows(&[]).is_empty());
    }
}
