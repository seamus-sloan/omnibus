//! "Send to Kobo" family: the interactive [`SendToKoboButton`], its
//! per-row action helper, the File System Access write flow, and the pure
//! path-sanitizing helpers that nest the KEPUB under `<Author>/<Title>/`.

use dioxus::prelude::*;

// Only the non-mobile `SendToKoboButton` awaits the shared sleeper; the mobile
// action is a disabled placeholder, so gate the import to match.
#[cfg(not(feature = "mobile"))]
use crate::platform_sleep::async_sleep_ms;

/// "Send to Kobo" CTA. Web/SSR renders the interactive
/// [`SendToKoboButton`]; mobile renders a disabled placeholder (the copy-over-
/// USB flow is desktop-only). The cfg gate lives at the helper definition (rule
/// 07: keep cfg out of rsx bodies), and SSR + first WASM paint emit the same
/// enabled button so hydration holds.
#[cfg(not(feature = "mobile"))]
pub(super) fn send_to_kobo_action(uuid: &str, book_author: &str, book_title: &str) -> Element {
    rsx! {
        SendToKoboButton {
            uuid: uuid.to_string(),
            book_author: book_author.to_string(),
            book_title: book_title.to_string(),
        }
    }
}

#[cfg(feature = "mobile")]
pub(super) fn send_to_kobo_action(_uuid: &str, _book_author: &str, _book_title: &str) -> Element {
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
        {super::send_result_toast("kobo", result)}
    }
}

/// True when `name`'s base (the part before any extension) is a Windows
/// reserved device name (case-insensitive) — `CON`, `PRN`, `AUX`, `NUL`,
/// `COM1`–`COM9`, `LPT1`–`LPT9`. Creating a folder or file with one of these
/// names fails on Windows even though FAT itself allows it.
#[cfg(not(feature = "mobile"))]
fn is_windows_reserved_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(base.strip_prefix("COM").or_else(|| base.strip_prefix("LPT")), Some(d) if matches!(d, "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"))
}

/// Sanitize one path component for a Kobo's FAT/exFAT filesystem: replace the
/// characters illegal on Windows/FAT (and control chars) with spaces, collapse
/// whitespace, strip stray leading/trailing dots and spaces, and defuse Windows
/// reserved device names. `None` when nothing usable survives. Pure —
/// unit-tested without a browser.
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
    if trimmed.is_empty() {
        return None;
    }
    // Underscore-prefix a reserved device name so the OS accepts it.
    if is_windows_reserved_name(trimmed) {
        Some(format!("_{trimmed}"))
    } else {
        Some(trimmed.to_string())
    }
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
    // Nest under <Author>/<Title>/ (each segment sanitized in the Rust caller
    // before interpolation), creating folders as needed, so the file lands in
    // the same layout Calibre and the Kobo use instead of a bare uuid file at
    // the drive root.
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

#[cfg(test)]
mod tests;
