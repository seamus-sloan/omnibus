use super::*;

const BASE: &str = "https://omni.test";
const TOKEN: &str = "tok123";

#[test]
fn resources_for_repoints_exactly_the_overridden_keys_at_this_server() {
    let map = resources_for(BASE, TOKEN);

    for key in OVERRIDDEN_KEYS {
        let value = map[key].as_str().unwrap_or_else(|| panic!("{key} missing"));
        assert!(
            value.starts_with(BASE),
            "{key} should point at this server, got {value}"
        );
    }
}

#[test]
fn resources_for_carries_the_path_token_on_the_device_facing_urls() {
    let map = resources_for(BASE, TOKEN);

    // `image_host` and `reading_services_host` are bare origins by protocol —
    // the device appends the templated path itself — so only the templated
    // entries carry the token.
    for key in [
        "image_url_template",
        "image_url_quality_template",
        "library_sync",
        "get_tests_request",
        "post_analytics_event",
    ] {
        assert!(
            map[key].as_str().unwrap().contains(TOKEN),
            "{key} should carry the path token"
        );
    }
}

#[test]
fn resources_for_leaves_the_store_long_tail_pointed_at_kobo() {
    // The whole point of pass-through: store browse/search/recommendations
    // keep working because the device talks to Kobo directly. A rewrite that
    // over-reached would break them silently, so spot-check the long tail.
    let map = resources_for(BASE, TOKEN);

    for key in [
        "products",
        "autocomplete",
        "product_recommendations",
        "store_search",
        "user_profile",
        "user_wishlist",
    ] {
        let value = map[key].as_str().unwrap_or_else(|| panic!("{key} missing"));
        assert!(
            value.contains("kobo.com"),
            "{key} should still point at Kobo, got {value}"
        );
        assert!(
            !value.contains(BASE),
            "{key} should not have been rewritten"
        );
    }
}

#[test]
fn resources_for_overrides_nothing_beyond_the_declared_key_set() {
    // Guards against an override being added to `resources_for` without being
    // declared in `OVERRIDDEN_KEYS` — the list is what the review reads.
    let map = resources_for(BASE, TOKEN);

    let rewritten: Vec<&String> = map
        .iter()
        .filter(|(_, v)| v.as_str().is_some_and(|s| s.starts_with(BASE)))
        .map(|(k, _)| k)
        .collect();

    assert_eq!(rewritten.len(), OVERRIDDEN_KEYS.len());
    for key in OVERRIDDEN_KEYS {
        assert!(rewritten.iter().any(|k| k.as_str() == key), "{key} missing");
    }
}

#[test]
fn resources_for_preserves_the_maps_full_key_set() {
    // Overriding must replace values, never drop or add keys — a device that
    // finds a key absent falls back to a hardcoded default and silently stops
    // using the server.
    let untouched = RESOURCES.len();

    assert_eq!(resources_for(BASE, TOKEN).len(), untouched);
}

#[test]
fn resources_for_emits_relative_overrides_when_the_origin_is_unknown() {
    // Mirrors the download/image URL builders: an empty origin yields
    // host-relative URLs rather than an invalid `http:///…`.
    let map = resources_for("", TOKEN);

    assert_eq!(
        map["library_sync"].as_str().unwrap(),
        format!("/kobo/{TOKEN}/v1/library/sync")
    );
}

#[test]
fn resources_map_retains_the_non_string_entries() {
    // Two upstream entries are objects, not strings. Round-tripping them
    // through the JSON asset must not flatten them into strings.
    let map = resources_for(BASE, TOKEN);

    assert!(map["blackstone_header"].is_object());
    assert!(map["free_books_page"].is_object());
}
