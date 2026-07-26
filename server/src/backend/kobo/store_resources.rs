//! Kobo's `v1/initialization` resources map, and the small set of entries
//! Omnibus redirects at itself.
//!
//! The map ships verbatim from Kobo: a device that reads it keeps talking to
//! Kobo for store browse, search, and recommendations, so the server never
//! becomes a party to that traffic. Only the entries listed in
//! [`OVERRIDDEN_KEYS`] point back here.

use std::sync::LazyLock;

use serde_json::{Map, Value};

/// The upstream map, kept as a JSON asset so it diffs as data rather than as
/// Rust. Parsed once — a malformed asset is a build-time authoring error, not
/// a runtime condition, so the panic message names the file.
static RESOURCES: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    let raw = include_str!("store_resources.json");
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => map,
        _ => panic!("kobo/store_resources.json is not a JSON object"),
    }
});

/// The resource keys Omnibus answers itself. Everything else stays pointed at
/// Kobo. Kept as a named list so a test can assert the override set exactly and
/// catch an accidental widening.
pub const OVERRIDDEN_KEYS: [&str; 7] = [
    "image_host",
    "image_url_template",
    "image_url_quality_template",
    "library_sync",
    "get_tests_request",
    "post_analytics_event",
    "reading_services_host",
];

/// Build the `Resources` map for one device: Kobo's map with
/// [`OVERRIDDEN_KEYS`] repointed at `base` (this server's origin) under the
/// caller's path `token`.
///
/// `base` is the already-resolved request origin; an empty `base` yields
/// host-relative overrides rather than an invalid `http:///…`, matching the
/// download/image URL builders.
pub fn resources_for(base: &str, token: &str) -> Map<String, Value> {
    let mut map = RESOURCES.clone();
    let prefix = format!("{base}/kobo/{token}");

    // Driven by `OVERRIDDEN_KEYS` rather than an inline list, so the declared
    // set and the applied set cannot drift apart.
    for key in OVERRIDDEN_KEYS {
        map.insert(
            key.to_string(),
            Value::String(override_value(key, base, &prefix)),
        );
    }
    map
}

/// The replacement value for one overridden key. Panics on an unknown key,
/// which is unreachable: the only caller iterates [`OVERRIDDEN_KEYS`], so a new
/// entry there without a match arm fails the first test that renders the map.
fn override_value(key: &str, base: &str, prefix: &str) -> String {
    match key {
        // Bare origins by protocol — the device appends the templated path.
        "image_host" | "reading_services_host" => base.to_string(),
        "image_url_template" => {
            format!("{prefix}/v1/books/{{ImageId}}/thumbnail/{{Width}}/{{Height}}/false/image.jpg")
        }
        "image_url_quality_template" => format!(
            "{prefix}/v1/books/{{ImageId}}/thumbnail/{{Width}}/{{Height}}/{{Quality}}/{{IsGreyscale}}/image.jpg"
        ),
        "library_sync" => format!("{prefix}/v1/library/sync"),
        "get_tests_request" => format!("{prefix}/v1/analytics/gettests"),
        "post_analytics_event" => format!("{prefix}/v1/analytics/event"),
        other => unreachable!("no override defined for declared key {other}"),
    }
}

#[cfg(test)]
mod tests;
