//! Shared JS-interop mechanics reused by the mobile/web JS-bridge modules
//! (barcode scanner, mobile audio, mobile reader): encoding a value as a JS
//! literal, and draining a persistent `dioxus::document::Eval` channel of
//! typed JS→Rust events. Each call site still builds and installs its own
//! bespoke shim script — the shapes differ too much per surface to share —
//! but all three reuse these two mechanical pieces instead of re-deriving
//! them independently.

use dioxus::document::Eval;

/// Encode `value` as a JS literal, falling back to the given raw JS
/// `fallback` (e.g. `"null"` or `"[]"`) on the unreachable-in-practice
/// encode failure.
pub fn json_literal_or<T: serde::Serialize + ?Sized>(value: &T, fallback: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| fallback.to_string())
}

/// [`json_literal_or`] with a `null` fallback — the common case.
pub fn json_literal<T: serde::Serialize + ?Sized>(value: &T) -> String {
    json_literal_or(value, "null")
}

/// Drain a persistent JS→Rust event channel until it closes (the surface was
/// torn down or navigated away from), invoking `handle` for each typed event.
pub async fn drain_events<T, F>(mut eval: Eval, mut handle: F)
where
    T: serde::de::DeserializeOwned,
    F: FnMut(T),
{
    while let Ok(event) = eval.recv::<T>().await {
        handle(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_literal_encodes_value_as_json() {
        assert_eq!(json_literal("hi"), "\"hi\"");
        assert_eq!(json_literal(&42), "42");
    }

    #[test]
    fn json_literal_or_uses_the_json_encoding_when_serialization_succeeds() {
        assert_eq!(json_literal_or("x", "null"), "\"x\"");
    }

    #[test]
    fn json_literal_or_falls_back_when_encoding_fails() {
        // A non-finite float is the one practical way `serde_json` refuses to
        // encode a value (JSON has no NaN/Infinity literal).
        assert_eq!(json_literal_or(&f64::NAN, "0"), "0");
    }
}
