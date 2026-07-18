//! Native share-sheet bridge for the mobile shell. WKWebView exposes no
//! usable Web Share API, so the reader's Share actions forward here through
//! the JS event channel and iOS presents a real `UIActivityViewController`.
//! Android keeps the in-WebView Web Share path ([`supported`] is `false`).

/// Whether this build bridges shares natively. The JS glue only installs the
/// `__omnibusOnShare*` shims when this is true; otherwise it keeps the
/// in-WebView Web Share / clipboard fallbacks.
pub(crate) fn supported() -> bool {
    cfg!(target_os = "ios")
}

/// Present the OS share sheet for a plain text snippet. No-op off-iOS.
pub(crate) fn share_text(text: &str) {
    if text.is_empty() {
        return;
    }
    #[cfg(target_os = "ios")]
    ios::share_text(text);
    #[cfg(not(target_os = "ios"))]
    tracing::warn!("native_share::share_text called on unsupported platform");
}

/// Decode a `data:image/png;base64,…` URL, stage it as a temp file, and
/// present the OS share sheet for it. No-op off-iOS.
pub(crate) fn share_png_data_url(name: &str, data_url: &str) {
    let Some(bytes) = decode_png_data_url(data_url) else {
        tracing::warn!("share_png_data_url: payload is not a base64 PNG data URL");
        return;
    };
    // Stage under the sandbox tmp dir; the OS reaps it, and sharing needs a
    // file URL (UIActivityViewController previews NSURL items as images).
    let path = std::env::temp_dir().join(sanitize_filename(name));
    if let Err(e) = std::fs::write(&path, bytes) {
        tracing::warn!(error = %e, "share_png_data_url: temp write failed");
        return;
    }
    #[cfg(target_os = "ios")]
    ios::share_file(&path);
    #[cfg(not(target_os = "ios"))]
    tracing::warn!(
        ?path,
        "native_share::share_png_data_url on unsupported platform"
    );
}

/// Upper bound on the base64 payload (~12 MiB → ~9 MiB PNG). A rendered
/// quote card is a few hundred KiB; the cap only exists because the reader
/// mounts book content with `allowScriptedContent`, so a hostile EPUB could
/// otherwise call the share shim with an arbitrarily large data URL and buy
/// an OOM / disk-fill for free.
const MAX_PNG_B64_LEN: usize = 12 * 1024 * 1024;

/// Extract the raw bytes from a base64 PNG data URL; `None` when the string
/// isn't one or exceeds [`MAX_PNG_B64_LEN`].
fn decode_png_data_url(data_url: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let b64 = data_url.strip_prefix("data:image/png;base64,")?;
    if b64.len() > MAX_PNG_B64_LEN {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

/// Longest sanitized filename we stage (chars); beyond book-title reason and
/// under every filesystem's component limit.
const MAX_FILENAME_LEN: usize = 80;

/// Keep the staged temp filename to a safe charset and length (the quote-card
/// name is derived from the book title, which can hold path separators and
/// run arbitrarily long). Truncation preserves the `.png` suffix.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        return "quote.png".into();
    }
    if cleaned.chars().count() <= MAX_FILENAME_LEN {
        return cleaned;
    }
    let stem: String = cleaned
        .chars()
        .take(MAX_FILENAME_LEN.saturating_sub(4))
        .collect();
    format!("{}.png", stem.trim_end_matches('.'))
}

#[cfg(target_os = "ios")]
mod ios {
    //! UIKit presentation. Everything must run on the main thread; the reader
    //! event loop already does (dioxus native drives the vdom there), and the
    //! `MainThreadMarker` guard turns a violation into a logged no-op instead
    //! of UB.

    use std::path::Path;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{NSArray, NSString, NSURL};
    use objc2_ui_kit::{UIActivityViewController, UIApplication};

    pub(super) fn share_text(text: &str) {
        let item = Retained::into_super(Retained::into_super(NSString::from_str(text)));
        present(vec![item]);
    }

    pub(super) fn share_file(path: &Path) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        present(vec![Retained::into_super(Retained::into_super(url))]);
    }

    /// Present a `UIActivityViewController` over the root view controller.
    fn present(items: Vec<Retained<AnyObject>>) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("native share skipped: not on the main thread");
            return;
        };
        let refs: Vec<&AnyObject> = items.iter().map(|i| &**i).collect();
        let activity_items = NSArray::from_slice(&refs);

        #[allow(deprecated)] // keyWindow: fine for a single-scene, single-window app
        let Some(root) = UIApplication::sharedApplication(mtm)
            .keyWindow()
            .and_then(|w| w.rootViewController())
        else {
            tracing::warn!("native share skipped: no root view controller");
            return;
        };

        // SAFETY: called on the main thread per `mtm`'s `MainThreadMarker` proof above;
        // `activity_items` and `alloc(mtm)` are both valid Objective-C objects owned for
        // the duration of this call.
        let avc = unsafe {
            UIActivityViewController::initWithActivityItems_applicationActivities(
                UIActivityViewController::alloc(mtm),
                &activity_items,
                None,
            )
        };
        // iPad presents this as a popover and traps without an anchor; anchor
        // to the root view so the sheet works on every device class.
        if let Some(pop) = avc.popoverPresentationController() {
            let view = root.view();
            pop.setSourceView(view.as_deref());
            if let Some(v) = view {
                pop.setSourceRect(v.bounds());
            }
        }
        root.presentViewController_animated_completion(&avc, true, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_png_data_url_roundtrips_base64_payload() {
        use base64::Engine as _;
        let bytes = vec![0x89u8, b'P', b'N', b'G', 0, 1, 2, 3];
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );
        assert_eq!(decode_png_data_url(&url), Some(bytes));
    }

    #[test]
    fn decode_png_data_url_rejects_non_png_payloads() {
        assert_eq!(decode_png_data_url("data:image/jpeg;base64,AAAA"), None);
        assert_eq!(decode_png_data_url("not a data url"), None);
        assert_eq!(decode_png_data_url("data:image/png;base64,@@@"), None);
    }

    #[test]
    fn decode_png_data_url_rejects_oversized_payloads() {
        let big = format!("data:image/png;base64,{}", "A".repeat(MAX_PNG_B64_LEN + 4));
        assert_eq!(decode_png_data_url(&big), None);
    }

    #[test]
    fn sanitize_filename_strips_path_separators_and_defaults() {
        assert_eq!(sanitize_filename("a/b\\c quote.png"), "a-b-c-quote.png");
        assert_eq!(sanitize_filename(""), "quote.png");
    }

    #[test]
    fn sanitize_filename_truncates_long_names_keeping_png_suffix() {
        let long = format!("{}.png", "x".repeat(300));
        let out = sanitize_filename(&long);
        assert!(out.chars().count() <= MAX_FILENAME_LEN);
        assert!(out.ends_with(".png"));
    }
}
