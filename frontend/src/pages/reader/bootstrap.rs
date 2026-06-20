//! Reader bootstrap: builds the IIFE that polls for `window.OmnibusReader`
//! and `window.ePub`, then calls `OmnibusReader.init(...)` with the chosen
//! CFI and typography. Extracted from `BookReadPage` so the parent reads as
//! plain Rust glue rather than JS template literals.

/// Inputs to [`reader_bootstrap_js`]; all string values are JSON-quoted
/// literals already (e.g. `"\"dark\""`, `"null"`), not raw values, so
/// the caller can feed any string through `serde_json::to_string`.
#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub(crate) struct BootstrapArgs<'a> {
    pub url_lit: &'a str,
    pub cfi_arg: &'a str,
    pub font_size: i32,
    pub theme_lit: &'a str,
    pub font_family_lit: &'a str,
    pub line_height_lit: &'a str,
    pub max_width_lit: &'a str,
    pub justify_val: bool,
}

/// Build the JS IIFE that mounts the reader once the vendored scripts are
/// ready. Polls up to 200 x 50 ms (~10 s) before signalling `error` via
/// `window.__omnibusOnStatus`.
#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub(crate) fn reader_bootstrap_js(args: &BootstrapArgs<'_>) -> String {
    let BootstrapArgs {
        url_lit,
        cfi_arg,
        font_size,
        theme_lit,
        font_family_lit,
        line_height_lit,
        max_width_lit,
        justify_val,
    } = *args;
    format!(
        r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusReader && window.ePub) {{ window.OmnibusReader.init("omnibus-viewer", {url_lit}, {{ cfi: {cfi_arg}, fontSize: {font_size}, theme: {theme_lit}, fontFamily: {font_family_lit}, lineHeight: {line_height_lit}, maxWidth: {max_width_lit}, justify: {justify_val} }}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else if (typeof window.__omnibusOnStatus === "function") {{ window.__omnibusOnStatus("error"); }} }})(); }})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_bootstrap_js_contains_init_call_and_polls_for_globals() {
        let js = reader_bootstrap_js(&BootstrapArgs {
            url_lit: "\"/api/ebooks/x/file\"",
            cfi_arg: "null",
            font_size: 18,
            theme_lit: "\"dark\"",
            font_family_lit: "null",
            line_height_lit: "null",
            max_width_lit: "null",
            justify_val: false,
        });
        assert!(js.contains("window.OmnibusReader.init"));
        assert!(js.contains("window.ePub"));
        assert!(js.contains("fontSize: 18"));
        assert!(js.contains("theme: \"dark\""));
        assert!(js.contains("justify: false"));
        assert!(js.contains("__omnibusOnStatus"));
    }

    #[test]
    fn reader_bootstrap_js_threads_typography_literals_through() {
        let js = reader_bootstrap_js(&BootstrapArgs {
            url_lit: "\"u\"",
            cfi_arg: "\"epubcfi(/6/2)\"",
            font_size: 22,
            theme_lit: "\"sepia\"",
            font_family_lit: "\"Georgia, serif\"",
            line_height_lit: "\"1.5\"",
            max_width_lit: "\"42rem\"",
            justify_val: true,
        });
        assert!(js.contains("cfi: \"epubcfi(/6/2)\""));
        assert!(js.contains("fontFamily: \"Georgia, serif\""));
        assert!(js.contains("lineHeight: \"1.5\""));
        assert!(js.contains("maxWidth: \"42rem\""));
        assert!(js.contains("justify: true"));
    }
}
