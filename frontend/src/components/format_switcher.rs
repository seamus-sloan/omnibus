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
    // Raw author + title, used to nest the Kobo write under `<Author>/<Title>/`
    // (see [`SendToKoboButton`]). Default empty → write at the drive root.
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
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
                                book_author: book_author.clone(),
                                book_title: book_title.clone(),
                            }
                        }
                    } else {
                        rsx! {
                            FormatRow {
                                key: "{row.label()}",
                                kind: row,
                                uuid: uuid.clone(),
                                book_author: book_author.clone(),
                                book_title: book_title.clone(),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FormatRow(
    kind: FormatKind,
    uuid: String,
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
) -> Element {
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
                        {send_to_kobo_action(&uuid, &book_author, &book_title)}
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
fn MultiFileRow(
    kind: FormatKind,
    uuid: String,
    files: Vec<BookFileInfo>,
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
) -> Element {
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
                div { class: "format-actions", {send_to_kobo_action(&uuid, &book_author, &book_title)} }
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

/// F4.1 "Send to Kobo" CTA. Web/SSR renders the interactive
/// [`SendToKoboButton`]; mobile renders a disabled placeholder (the copy-over-
/// USB flow is desktop-only). The cfg gate lives at the helper definition (rule
/// 07: keep cfg out of rsx bodies), and SSR + first WASM paint emit the same
/// enabled button so hydration holds.
#[cfg(not(feature = "mobile"))]
fn send_to_kobo_action(uuid: &str, book_author: &str, book_title: &str) -> Element {
    rsx! {
        SendToKoboButton {
            uuid: uuid.to_string(),
            book_author: book_author.to_string(),
            book_title: book_title.to_string(),
        }
    }
}

#[cfg(feature = "mobile")]
fn send_to_kobo_action(_uuid: &str, _book_author: &str, _book_title: &str) -> Element {
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

/// Interactive "Send to Kobo" button. When the browser supports the File System
/// Access API (Chrome/Edge), clicking writes the book's KEPUB straight onto a
/// plugged-in Kobo: the device mounts as a USB drive on the *client* machine
/// (never the server), so the write happens in the browser. The chosen
/// directory handle is remembered in IndexedDB, so after the first pick a click
/// writes silently. Browsers without the API fall back to a plain download.
/// Shows "Sending…" in-place and raises a toast on the terminal outcome —
/// success/download auto-dismisses, errors stay until dismissed. `class` /
/// `testid` default to the per-format-row styling; the hero CTA overrides them.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn SendToKoboButton(
    uuid: String,
    // Raw book author + title — nest the written file under
    // `<Author>/<Title>/` so it lands in the same folder layout Calibre and the
    // Kobo use, instead of a bare uuid file at the drive root. Empty → root.
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
    #[props(default = "btn".to_string())] class: String,
    #[props(default = "action-kobo".to_string())] testid: String,
) -> Element {
    let mut in_flight = use_signal(|| false);
    // (is_error, message) — None until a send completes / the toast is dismissed.
    let mut result = use_signal(|| None::<(bool, String)>);
    // Monotonic id of the latest send. A superseded task must not touch shared
    // state — otherwise an earlier send's auto-dismiss sleep can clear (or hide
    // the error of) a newer send's toast.
    let mut send_seq = use_signal(|| 0u64);

    rsx! {
        button {
            class: "{class}",
            r#type: "button",
            disabled: in_flight(),
            title: "Write the KEPUB onto a plugged-in Kobo (Chrome/Edge), or download it to copy over",
            "data-testid": "{testid}",
            onclick: move |_| {
                let uuid = uuid.clone();
                let subdir = kobo_subdir(&book_author, &book_title);
                let seq = *send_seq.peek() + 1;
                send_seq.set(seq);
                in_flight.set(true);
                result.set(None);
                spawn(async move {
                    let outcome = write_kepub_to_kobo(&uuid, subdir.as_deref()).await;
                    // A newer send has superseded this one — leave all shared
                    // state to it.
                    if *send_seq.peek() != seq {
                        return;
                    }
                    in_flight.set(false);
                    // `None` = the user cancelled the directory picker; stay quiet.
                    if let Some((is_error, message)) = outcome {
                        result.set(Some((is_error, message)));
                        if !is_error {
                            async_sleep_ms(5000).await;
                            // Only clear if we're still the latest send.
                            if *send_seq.peek() == seq {
                                result.set(None);
                            }
                        }
                    }
                });
            },
            if in_flight() { "Sending\u{2026}" } else { "Send to Kobo" }
        }
        if let Some((is_error, message)) = result() {
            div { class: "kobo-toast card", role: "status",
                span {
                    "data-testid": "kobo-send-status",
                    class: if is_error { "kobo-toast-msg error" } else { "kobo-toast-msg success" },
                    "{message}"
                }
                button {
                    class: "btn ghost sm",
                    "data-testid": "kobo-toast-dismiss",
                    aria_label: "Dismiss",
                    onclick: move |_| result.set(None),
                    "\u{00d7}"
                }
            }
        }
    }
}

/// Sanitize one path component for a Kobo's FAT/exFAT filesystem: replace the
/// characters illegal on Windows/FAT (and control chars) with spaces, collapse
/// whitespace, and strip stray leading/trailing dots and spaces. `None` when
/// nothing usable survives. Pure — unit-tested without a browser.
#[cfg(not(feature = "mobile"))]
fn kobo_path_segment(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // FAT caps a component at 255 bytes; stay well under and cut on a char
    // boundary, then re-trim in case the cut left a trailing dot/space.
    let capped: String = collapsed.chars().take(120).collect();
    let trimmed = capped.trim_matches(|c| c == '.' || c == ' ');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Build the `<Author>/<Title>` subfolder a Kobo write nests the file under, or
/// `None` (write at the drive root) when neither segment survives sanitization.
/// Segments join with `/`; the JS write flow splits on it to create each
/// directory. The file *inside* keeps the uuid name so a future USB annotation
/// import can still map the device's `ContentID` back to the book.
#[cfg(not(feature = "mobile"))]
fn kobo_subdir(book_author: &str, book_title: &str) -> Option<String> {
    let segs: Vec<String> = [book_author, book_title]
        .iter()
        .filter_map(|s| kobo_path_segment(s))
        .collect();
    (!segs.is_empty()).then(|| segs.join("/"))
}

/// Map the JS write flow's status string to the toast `(is_error, message)`
/// pair, or `None` when the user cancelled the picker (no toast at all). Pure,
/// so it's unit-tested without a browser. Only *called* on web (the SSR/native
/// stub of `write_kepub_to_kobo` never runs it), so the non-web lib build sees
/// it as dead outside its tests — allow that rather than gate it off `web` and
/// lose the server-feature test coverage.
#[cfg(not(feature = "mobile"))]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
fn kobo_outcome(status: &str, message: Option<String>) -> Option<(bool, String)> {
    match status {
        "ok" => Some((
            false,
            "Sent to your Kobo \u{2014} eject it safely before unplugging.".to_string(),
        )),
        "downloaded" => Some((
            false,
            "Your browser can\u{2019}t write to the device, so the KEPUB downloaded instead \u{2014} drag it onto your Kobo.".to_string(),
        )),
        "cancelled" => None,
        _ => Some((
            true,
            format!(
                "Send to Kobo failed: {}",
                message.unwrap_or_else(|| "unknown error".to_string())
            ),
        )),
    }
}

/// Outcome pushed back from [`KOBO_WRITE_JS`] over the eval channel.
#[cfg(all(not(feature = "mobile"), feature = "web"))]
#[derive(serde::Deserialize)]
struct KoboWriteOutcome {
    status: String,
    #[serde(default)]
    message: Option<String>,
}

/// Run the File System Access write flow and map its result to a toast pair.
/// Same-origin `fetch` carries the session cookie, so the KEPUB endpoint
/// authenticates like any other in-page request. Web-only; the SSR stub below
/// never runs (the click handler only fires in the browser) but must compile.
#[cfg(all(not(feature = "mobile"), feature = "web"))]
async fn write_kepub_to_kobo(uuid: &str, subdir: Option<&str>) -> Option<(bool, String)> {
    // Interpolate as JS string literals so neither can break out of the script
    // (the uuid is a UUIDv4 and the subdir is already sanitized, but quote
    // defensively).
    let uuid_lit = serde_json::to_string(uuid).unwrap_or_else(|_| "\"\"".to_string());
    let subdir_lit =
        serde_json::to_string(subdir.unwrap_or("")).unwrap_or_else(|_| "\"\"".to_string());
    let js = KOBO_WRITE_JS
        .replace("__UUID__", &uuid_lit)
        .replace("__SUBDIR__", &subdir_lit);
    let mut eval = dioxus::document::eval(&js);
    match eval.recv::<KoboWriteOutcome>().await {
        Ok(out) => kobo_outcome(&out.status, out.message),
        Err(_) => Some((true, "Send to Kobo failed.".to_string())),
    }
}

#[cfg(all(not(feature = "mobile"), not(feature = "web"), feature = "server"))]
async fn write_kepub_to_kobo(_uuid: &str, _subdir: Option<&str>) -> Option<(bool, String)> {
    None
}

/// Browser-side write flow. Reuses a remembered Kobo directory handle
/// (persisted in IndexedDB) or prompts for one once, fetches the KEPUB, and
/// writes it under `<Author>/<Title>/` on the device (creating the folders) —
/// Kobo imports files anywhere on the drive, so this never touches the device's
/// `KoboReader.sqlite` master DB. Falls back to a plain download when the File
/// System Access API is absent (Firefox/Safari). `__UUID__` and `__SUBDIR__`
/// are substituted with quoted JS string literals before eval.
#[cfg(all(not(feature = "mobile"), feature = "web"))]
const KOBO_WRITE_JS: &str = r#"
const uuid = __UUID__;
const subdir = __SUBDIR__;
const idb = () => new Promise((res, rej) => {
  const r = indexedDB.open('omnibus-kobo', 1);
  r.onupgradeneeded = () => r.result.createObjectStore('handles');
  r.onsuccess = () => res(r.result);
  r.onerror = () => rej(r.error);
});
const openDir = async () => {
  try {
    const db = await idb();
    return await new Promise((res, rej) => {
      const rq = db.transaction('handles', 'readonly').objectStore('handles').get('dir');
      rq.onsuccess = () => res(rq.result || null);
      rq.onerror = () => rej(rq.error);
    });
  } catch (_) { return null; }
};
const saveDir = async (h) => {
  try {
    const db = await idb();
    await new Promise((res, rej) => {
      const tx = db.transaction('handles', 'readwrite');
      tx.objectStore('handles').put(h, 'dir');
      tx.oncomplete = () => res();
      tx.onerror = () => rej(tx.error);
    });
  } catch (_) {}
};
await (async () => {
  try {
    if (!window.showDirectoryPicker) {
      const a = document.createElement('a');
      a.href = `/api/ebooks/${uuid}/kepub`;
      a.download = '';
      document.body.appendChild(a);
      a.click();
      a.remove();
      dioxus.send({ status: 'downloaded' });
      return;
    }
    let dir = await openDir();
    if (dir) {
      let perm = await dir.queryPermission({ mode: 'readwrite' });
      if (perm !== 'granted') perm = await dir.requestPermission({ mode: 'readwrite' });
      if (perm !== 'granted') dir = null;
    }
    if (!dir) {
      dir = await window.showDirectoryPicker({ id: 'omnibus-kobo', mode: 'readwrite', startIn: 'desktop' });
      await saveDir(dir);
    }
    const resp = await fetch(`/api/ebooks/${uuid}/kepub`, { credentials: 'include' });
    if (!resp.ok) { dioxus.send({ status: 'error', message: `download failed (${resp.status})` }); return; }
    const cd = resp.headers.get('content-disposition') || '';
    const m = /filename="?([^"]+)"?/.exec(cd);
    const filename = (m && m[1]) || `${uuid}.kepub.epub`;
    const blob = await resp.blob();
    // Nest under <Author>/<Title>/ (each segment pre-sanitized server-side),
    // creating folders as needed, so the file lands in the same layout Calibre
    // and the Kobo use instead of a bare uuid file at the drive root.
    let target = dir;
    if (subdir) {
      for (const seg of subdir.split('/')) {
        if (seg) target = await target.getDirectoryHandle(seg, { create: true });
      }
    }
    const fh = await target.getFileHandle(filename, { create: true });
    const w = await fh.createWritable();
    await w.write(blob);
    await w.close();
    dioxus.send({ status: 'ok', filename: filename });
  } catch (e) {
    if (e && e.name === 'AbortError') { dioxus.send({ status: 'cancelled' }); return; }
    dioxus.send({ status: 'error', message: (e && e.message) || String(e) });
  }
})();
"#;

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

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_outcome_reports_success_for_device_write() {
        let (is_error, msg) = super::kobo_outcome("ok", None).unwrap();
        assert!(!is_error);
        assert!(msg.contains("Sent to your Kobo"));
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_outcome_reports_download_fallback_for_unsupported_browser() {
        let (is_error, msg) = super::kobo_outcome("downloaded", None).unwrap();
        assert!(!is_error);
        assert!(msg.contains("downloaded instead"));
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_outcome_stays_silent_when_picker_cancelled() {
        assert!(super::kobo_outcome("cancelled", None).is_none());
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_outcome_surfaces_error_message_when_present() {
        let (is_error, msg) = super::kobo_outcome("error", Some("disk full".into())).unwrap();
        assert!(is_error);
        assert_eq!(msg, "Send to Kobo failed: disk full");
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_outcome_falls_back_to_unknown_error_without_message() {
        let (is_error, msg) = super::kobo_outcome("error", None).unwrap();
        assert!(is_error);
        assert_eq!(msg, "Send to Kobo failed: unknown error");
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_path_segment_replaces_illegal_chars_and_collapses_space() {
        assert_eq!(
            super::kobo_path_segment("AC/DC: Back \"in\"  Black?").as_deref(),
            Some("AC DC Back in Black"),
        );
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_path_segment_strips_leading_trailing_dots_and_spaces() {
        assert_eq!(
            super::kobo_path_segment("  .Hidden Title. ").as_deref(),
            Some("Hidden Title"),
        );
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_path_segment_returns_none_when_nothing_usable_remains() {
        assert_eq!(super::kobo_path_segment("   ///  ").as_deref(), None);
        assert_eq!(super::kobo_path_segment("").as_deref(), None);
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_path_segment_caps_length_on_a_char_boundary() {
        let seg = super::kobo_path_segment(&"é".repeat(200)).unwrap();
        assert_eq!(seg.chars().count(), 120);
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_subdir_joins_author_and_title() {
        assert_eq!(
            super::kobo_subdir("Ada Lovelace", "Notes on the Engine").as_deref(),
            Some("Ada Lovelace/Notes on the Engine"),
        );
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_subdir_uses_only_the_surviving_segment() {
        assert_eq!(
            super::kobo_subdir("", "Just a Title").as_deref(),
            Some("Just a Title"),
        );
        assert_eq!(
            super::kobo_subdir("Author Only", "   ").as_deref(),
            Some("Author Only"),
        );
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn kobo_subdir_is_none_when_both_segments_are_empty() {
        assert_eq!(super::kobo_subdir("  ", "").as_deref(), None);
    }

    #[test]
    fn empty_input_renders_nothing_meaningful() {
        // We don't exercise the actual rsx! macro (no SSR dep in this crate),
        // but the prepare_rows path is what gates the FormatSwitcher's
        // `rows.is_empty() → return rsx!{}` branch.
        assert!(prepare_rows(&[]).is_empty());
    }
}
