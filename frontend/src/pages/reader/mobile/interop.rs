//! Mobile reader interop: pure JS-string builders plus the
//! `dioxus::document::eval` seam that mounts the vendored epub.js glue in the
//! wry WebView. Installs shims that forward the glue's `window.__omnibusOn*`
//! callbacks into `dioxus.send(...)` (drained via `Eval::recv`). The two pure
//! builders are unit-tested; `install_reader_surface` is the eval seam.

use dioxus::document::Eval;

/// Absolute URLs of the vendored reader runtime, resolved from the `asset!`
/// bundle. Loaded in this order on mobile — `JSZip` first, since epub.js's UMD
/// binds `window.JSZip` at evaluation time.
pub(super) struct ReaderScripts {
    pub jszip: String,
    pub epub: String,
    pub glue: String,
}

/// Build the tokened, absolute EPUB-file URL epub.js fetches from the WebView.
/// That cross-origin XHR can carry neither a cookie nor a bearer header, so the
/// session token rides the `?token=` query (server side: `MediaAuthUser`).
pub(super) fn file_token_url(server_url: &str, uuid: &str, token: Option<&str>) -> String {
    let base = format!("{server_url}/api/ebooks/{uuid}/file");
    match token {
        Some(t) => format!("{base}?token={t}"),
        None => base,
    }
}

/// Build the install IIFE: define the `__omnibusOn*` → `dioxus.send` shims,
/// then load the vendored runtime **in order** (`JSZip` → epub.js → glue) and
/// mount the book. Unlike the web build (whose SSR emits ordered, parser-
/// inserted `<script>` tags), mobile has no ordering guarantee from
/// `document::Script`, so we chain `onload` promises here — epub.js binds
/// `window.JSZip` at evaluation time, so it must not run before JSZip exists.
/// Each script is skipped if its global is already present (SPA re-entry), and
/// each `load` has a 10 s timeout so a stalled request surfaces the error
/// overlay instead of hanging on "Loading…" forever. `opts` is any
/// JSON-serializable value shaped like the glue's `init` bag. `native_share`
/// installs the `__omnibusOnShare*` shims the glue prefers over the Web Share
/// API — only set where the host actually presents a sheet (iOS), so Android
/// keeps its in-WebView share path.
pub(super) fn install_surface_js(
    url: &str,
    opts: &serde_json::Value,
    scripts: &ReaderScripts,
    native_share: bool,
) -> String {
    let url_lit = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into());
    let opts_lit = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
    let jszip = serde_json::to_string(&scripts.jszip).unwrap_or_else(|_| "\"\"".into());
    let epub = serde_json::to_string(&scripts.epub).unwrap_or_else(|_| "\"\"".into());
    let glue = serde_json::to_string(&scripts.glue).unwrap_or_else(|_| "\"\"".into());
    let share_shims = if native_share {
        r#"
  window.__omnibusOnShareText=function(t){dioxus.send({kind:"ShareText",text:t});};
  window.__omnibusOnShareImage=function(j){dioxus.send({kind:"ShareImage",json:j});};"#
    } else {
        ""
    };
    format!(
        r#"(function(){{
  window.__omnibusOnRelocate=function(j){{dioxus.send({{kind:"Relocate",json:j}});}};
  window.__omnibusOnStatus=function(s){{dioxus.send({{kind:"Status",state:s}});}};
  window.__omnibusOnSelection=function(j){{dioxus.send({{kind:"Selection",json:j}});}};
  window.__omnibusOnSelectionCleared=function(){{dioxus.send({{kind:"SelectionCleared"}});}};
  window.__omnibusOnToc=function(j){{dioxus.send({{kind:"Toc",json:j}});}};
  window.__omnibusOnSearchResults=function(j){{dioxus.send({{kind:"Search",json:j}});}};{share_shims}
  function load(src){{return new Promise(function(res,rej){{
    var s=document.createElement("script");s.src=src;s.async=false;
    var t=setTimeout(function(){{rej();}},10000);
    s.onload=function(){{clearTimeout(t);res();}};
    s.onerror=function(){{clearTimeout(t);rej();}};
    document.head.appendChild(s);
  }});}}
  function ensure(has,src){{return has()?Promise.resolve():load(src);}}
  ensure(function(){{return !!window.JSZip;}},{jszip})
    .then(function(){{return ensure(function(){{return !!window.ePub;}},{epub});}})
    .then(function(){{return ensure(function(){{return !!window.OmnibusReader;}},{glue});}})
    .then(function(){{
      if(window.OmnibusReader&&window.ePub){{
        window.OmnibusReader.init("omnibus-viewer",{url_lit},{opts_lit});
      }}else{{dioxus.send({{kind:"Status",state:"error"}});}}
    }})
    .catch(function(){{dioxus.send({{kind:"Status",state:"error"}});}});
}})();"#
    )
}

/// Eval the install script and return the persistent [`Eval`] the caller drains
/// for reader events.
pub(super) fn install_reader_surface(
    url: &str,
    opts: &serde_json::Value,
    scripts: &ReaderScripts,
    native_share: bool,
) -> Eval {
    dioxus::document::eval(&install_surface_js(url, opts, scripts, native_share))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_token_url_appends_token_and_prefixes_origin() {
        assert_eq!(
            file_token_url("http://h:3000", "abc", Some("tok")),
            "http://h:3000/api/ebooks/abc/file?token=tok"
        );
        assert_eq!(
            file_token_url("http://h:3000", "abc", None),
            "http://h:3000/api/ebooks/abc/file"
        );
    }

    #[test]
    fn install_surface_js_defines_shims_and_calls_init_with_url_and_opts() {
        let opts = serde_json::json!({ "cfi": "epubcfi(/6/2)", "fontSize": 18, "theme": "sepia" });
        let scripts = ReaderScripts {
            jszip: "/assets/jszip.js".into(),
            epub: "/assets/epub.js".into(),
            glue: "/assets/glue.js".into(),
        };
        let js = install_surface_js("http://h/api/ebooks/x/file?token=t", &opts, &scripts, true);
        // Every glue callback is shimmed into the Dioxus eval channel.
        for cb in [
            "__omnibusOnRelocate",
            "__omnibusOnStatus",
            "__omnibusOnSelection",
            "__omnibusOnSelectionCleared",
            "__omnibusOnShareText",
            "__omnibusOnShareImage",
            "__omnibusOnToc",
            "__omnibusOnSearchResults",
        ] {
            assert!(js.contains(cb), "missing shim for {cb}");
        }
        assert!(js.contains("dioxus.send"));
        // Mounts against the shared viewer element with the tokened URL + opts.
        assert!(js.contains(r#"window.OmnibusReader.init("omnibus-viewer""#));
        assert!(js.contains(r#""http://h/api/ebooks/x/file?token=t""#));
        assert!(js.contains(r#""theme":"sepia""#));
        assert!(js.contains(r#""fontSize":18"#));
        // Loads the runtime JSZip-first (epub.js binds JSZip at eval time),
        // then guards init on the globals being present.
        let jz = js.find("/assets/jszip.js").expect("loads jszip");
        let ep = js.find("/assets/epub.js").expect("loads epub");
        let gl = js.find("/assets/glue.js").expect("loads glue");
        assert!(jz < ep && ep < gl, "runtime must load JSZip → epub → glue");
        // Each load is time-boxed so a stalled request can't hang init forever.
        assert!(js.contains("setTimeout") && js.contains("clearTimeout"));
        assert!(js.contains("window.OmnibusReader&&window.ePub"));
        assert!(js.contains(r#"{kind:"Status",state:"error"}"#));
        // No leaked `format!` escape pairs.
        assert!(!js.contains("{{"), "literal {{ leaked into JS");
        assert!(!js.contains("}}"), "literal }} leaked into JS");
    }

    #[test]
    fn install_surface_js_omits_share_shims_when_native_share_unsupported() {
        let scripts = ReaderScripts {
            jszip: "/a.js".into(),
            epub: "/b.js".into(),
            glue: "/c.js".into(),
        };
        let js = install_surface_js("u", &serde_json::json!({}), &scripts, false);
        // Android / other targets keep the glue's in-WebView Web Share path,
        // which only runs when these shims are absent.
        assert!(!js.contains("__omnibusOnShareText"));
        assert!(!js.contains("__omnibusOnShareImage"));
    }
}
