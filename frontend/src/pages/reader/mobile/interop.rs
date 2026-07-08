//! Mobile reader interop: pure JS-string builders plus the
//! `dioxus::document::eval` seam that mounts the vendored epub.js glue in the
//! wry WebView. Installs shims that forward the glue's `window.__omnibusOn*`
//! callbacks into `dioxus.send(...)` (drained via `Eval::recv`). The two pure
//! builders are unit-tested; `install_reader_surface` is the eval seam.

use dioxus::document::Eval;

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
/// poll ~10 s (200 × 50 ms) for the vendored globals, then mount the book.
/// Mirrors the web `reader_bootstrap_js` poll/timeout, but the callback sink is
/// the Dioxus eval channel rather than `window` closures. `opts` is any
/// JSON-serializable value shaped like the glue's `init` options bag.
pub(super) fn install_surface_js(url: &str, opts: &serde_json::Value) -> String {
    let url_lit = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into());
    let opts_lit = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
    format!(
        r#"(function(){{
  window.__omnibusOnRelocate=function(j){{dioxus.send({{kind:"Relocate",json:j}});}};
  window.__omnibusOnStatus=function(s){{dioxus.send({{kind:"Status",state:s}});}};
  window.__omnibusOnSelection=function(j){{dioxus.send({{kind:"Selection",json:j}});}};
  window.__omnibusOnToc=function(j){{dioxus.send({{kind:"Toc",json:j}});}};
  window.__omnibusOnSearchResults=function(j){{dioxus.send({{kind:"Search",json:j}});}};
  var n=0;(function go(){{
    if(window.OmnibusReader&&window.ePub){{
      window.OmnibusReader.init("omnibus-viewer",{url_lit},{opts_lit});
    }}else if(n++<200){{setTimeout(go,50);}}
    else{{dioxus.send({{kind:"Status",state:"error"}});}}
  }})();
}})();"#
    )
}

/// Eval the install script and return the persistent [`Eval`] the caller drains
/// for reader events.
pub(super) fn install_reader_surface(url: &str, opts: &serde_json::Value) -> Eval {
    dioxus::document::eval(&install_surface_js(url, opts))
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
        let js = install_surface_js("http://h/api/ebooks/x/file?token=t", &opts);
        // Every glue callback is shimmed into the Dioxus eval channel.
        for cb in [
            "__omnibusOnRelocate",
            "__omnibusOnStatus",
            "__omnibusOnSelection",
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
        // Polls for the vendored globals and errors out after the budget.
        assert!(js.contains("window.OmnibusReader&&window.ePub"));
        assert!(js.contains(r#"{kind:"Status",state:"error"}"#));
        // No leaked `format!` escape pairs.
        assert!(!js.contains("{{"), "literal {{ leaked into JS");
        assert!(!js.contains("}}"), "literal }} leaked into JS");
    }
}
