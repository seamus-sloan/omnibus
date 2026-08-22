//! Mobile counterpart of `typography::save_reader_pref` / `load_reader_pref`.
//! The wry WebView has a real `window.localStorage`, driven through the same
//! `dioxus::document::eval` seam `mobile::interop` uses rather than the web
//! build's `web_sys` API (native, not WASM). Writes are fire-and-forget; reads
//! are necessarily async, unlike the synchronous `web_sys::Storage` path.

use std::time::Duration;

use dioxus::document::eval;
use dioxus::prelude::WritableExt;

use super::super::prefs::{ReaderPrefs, FONT_SIZE_MAX, FONT_SIZE_MIN};
use super::super::typography::{LineSpacing, Margins, Spread, Typeface};

/// How long `load_and_apply_reader_prefs` waits for the bulk-read eval to
/// respond before giving up and leaving the caller's seeded defaults —
/// mirrors the 10 s script-load timeout in `mobile::interop::install_surface_js`.
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Every `omn.*` key persisted by [`ReaderPrefs`]'s setters, read back in one
/// round trip. Order doesn't matter — the bulk-read script keys its result
/// object by name.
const PREF_KEYS: [&str; 6] = [
    "omn.fontSize",
    "omn.typeface",
    "omn.lineSpacing",
    "omn.margins",
    "omn.justify",
    "omn.spread",
];

/// Build the JS that writes a single `key`/`value` pair to `localStorage`.
fn save_pref_js(key: &str, value: &str) -> String {
    let key_lit = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into());
    let value_lit = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    format!("localStorage.setItem({key_lit}, {value_lit});")
}

/// Fire-and-forget write of a single reader preference under its `omn.*`
/// key, mirroring [`super::super::typography::save_reader_pref`] but
/// through the JS-eval bridge instead of `web_sys`.
pub(crate) fn save_reader_pref_mobile(key: &str, value: &str) {
    let _ = eval(&save_pref_js(key, value));
}

/// Build the JS that bulk-reads every `omn.*` key and sends the present ones
/// back as a single JSON object, e.g. `{"omn.fontSize": "18"}` — missing
/// keys are omitted rather than sent as `null`.
fn load_all_js() -> String {
    let keys_lit = serde_json::to_string(&PREF_KEYS).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(function(){{ var out={{}}; {keys_lit}.forEach(function(k){{ var v=localStorage.getItem(k); if(v!==null) out[k]=v; }}); dioxus.send(out); }})();"#
    )
}

/// Raw JSON shape sent back by [`load_all_js`] — every field optional since
/// a fresh install has no `omn.*` keys yet.
#[derive(serde::Deserialize, Default)]
struct StoredPrefs {
    #[serde(rename = "omn.fontSize")]
    font_size: Option<String>,
    #[serde(rename = "omn.typeface")]
    typeface: Option<String>,
    #[serde(rename = "omn.lineSpacing")]
    line_spacing: Option<String>,
    #[serde(rename = "omn.margins")]
    margins: Option<String>,
    #[serde(rename = "omn.justify")]
    justify: Option<String>,
    #[serde(rename = "omn.spread")]
    spread: Option<String>,
}

/// [`StoredPrefs`] after each field is parsed through its own token format
/// (or dropped to `None` on an unrecognized/missing token). Kept separate
/// from the signal-writing caller so the parsing itself is unit-testable.
#[derive(Debug, Default, PartialEq)]
struct ParsedPrefs {
    font_size: Option<i32>,
    typeface: Option<Typeface>,
    line_spacing: Option<LineSpacing>,
    margins: Option<Margins>,
    justify: Option<bool>,
    spread: Option<Spread>,
}

fn parse_stored(stored: StoredPrefs) -> ParsedPrefs {
    ParsedPrefs {
        font_size: stored
            .font_size
            .and_then(|s| s.parse::<i32>().ok())
            .map(|n| n.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)),
        typeface: stored.typeface.and_then(|s| Typeface::from_storage(&s)),
        line_spacing: stored
            .line_spacing
            .and_then(|s| LineSpacing::from_storage(&s)),
        margins: stored.margins.and_then(|s| Margins::from_storage(&s)),
        justify: stored.justify.and_then(|s| match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }),
        spread: stored.spread.and_then(|s| Spread::from_storage(&s)),
    }
}

/// Load every persisted reader pref from the WebView's `localStorage` and
/// apply the present ones to `prefs`'s signals in place. Best-effort: an
/// eval/parse failure, or a `recv` that never resolves within
/// [`LOAD_TIMEOUT`], just leaves the caller's already-seeded defaults rather
/// than blocking `mount_and_drain` forever on "Loading…".
///
/// Timing tradeoff (AC3): unlike web's synchronous `web_sys` read, this eval
/// is necessarily async, so `mobile::mount_and_drain` awaits it *before*
/// building the glue's init options — the reader surface itself always
/// mounts with the persisted values on the happy path. What can still lag
/// one frame behind is any reader chrome that reads these signals before
/// this resolves (e.g. the AA panel, if opened during that window); there is
/// no synchronous mobile localStorage API to avoid that gap entirely.
pub(crate) async fn load_and_apply_reader_prefs(mut prefs: ReaderPrefs) {
    let mut ev = eval(&load_all_js());
    let Ok(Ok(stored)) = tokio::time::timeout(LOAD_TIMEOUT, ev.recv::<StoredPrefs>()).await else {
        return;
    };
    let parsed = parse_stored(stored);
    if let Some(n) = parsed.font_size {
        prefs.font_size.set(n);
    }
    if let Some(t) = parsed.typeface {
        prefs.typeface.set(t);
    }
    if let Some(ls) = parsed.line_spacing {
        prefs.line_spacing.set(ls);
    }
    if let Some(m) = parsed.margins {
        prefs.margins.set(m);
    }
    if let Some(j) = parsed.justify {
        prefs.justify.set(j);
    }
    if let Some(s) = parsed.spread {
        prefs.spread.set(s);
    }
}

#[cfg(test)]
mod tests;
