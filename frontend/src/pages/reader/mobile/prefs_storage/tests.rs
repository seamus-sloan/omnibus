//! Unit tests for [`super`]: JS-builder string shape and the
//! stored-token-to-typed-pref parsing table.

use super::*;

#[test]
fn save_pref_js_json_quotes_key_and_value_into_local_storage_set_item() {
    let js = save_pref_js("omn.typeface", "classic");
    assert_eq!(js, r#"localStorage.setItem("omn.typeface", "classic");"#);
}

#[test]
fn save_pref_js_escapes_a_value_containing_a_quote() {
    let js = save_pref_js("omn.justify", "tr\"ue");
    assert!(js.contains(r#""tr\"ue""#));
}

#[test]
fn load_all_js_reads_every_pref_key_and_sends_the_result() {
    let js = load_all_js();
    for key in PREF_KEYS {
        assert!(js.contains(key), "missing key {key} in bulk-read script");
    }
    assert!(js.contains("dioxus.send"));
    assert!(js.contains("localStorage.getItem"));
    // No leaked `format!` escape pairs.
    assert!(!js.contains("{{"), "literal {{ leaked into JS");
    assert!(!js.contains("}}"), "literal }} leaked into JS");
}

#[test]
fn parse_stored_returns_all_none_when_every_field_is_absent() {
    assert_eq!(parse_stored(StoredPrefs::default()), ParsedPrefs::default());
}

#[test]
fn parse_stored_parses_every_field_when_present() {
    let stored = StoredPrefs {
        font_size: Some("22".into()),
        typeface: Some("classic".into()),
        line_spacing: Some("airy".into()),
        margins: Some("wide".into()),
        justify: Some("true".into()),
        spread: Some("single".into()),
    };
    assert_eq!(
        parse_stored(stored),
        ParsedPrefs {
            font_size: Some(22),
            typeface: Some(Typeface::Classic),
            line_spacing: Some(LineSpacing::Airy),
            margins: Some(Margins::Wide),
            justify: Some(true),
            spread: Some(Spread::Single),
        }
    );
}

#[test]
fn parse_stored_drops_unrecognized_enum_tokens_to_none() {
    let stored = StoredPrefs {
        typeface: Some("nonsense".into()),
        line_spacing: Some("".into()),
        margins: Some("NARROW".into()),
        spread: Some("triple".into()),
        ..StoredPrefs::default()
    };
    let parsed = parse_stored(stored);
    assert_eq!(parsed.typeface, None);
    assert_eq!(parsed.line_spacing, None);
    assert_eq!(parsed.margins, None);
    assert_eq!(parsed.spread, None);
}

#[test]
fn parse_stored_clamps_an_out_of_range_font_size_to_the_slider_bounds() {
    let too_big = StoredPrefs {
        font_size: Some("999".into()),
        ..StoredPrefs::default()
    };
    assert_eq!(parse_stored(too_big).font_size, Some(FONT_SIZE_MAX));

    let too_small = StoredPrefs {
        font_size: Some("-5".into()),
        ..StoredPrefs::default()
    };
    assert_eq!(parse_stored(too_small).font_size, Some(FONT_SIZE_MIN));
}

#[test]
fn parse_stored_drops_an_unparseable_font_size_to_none() {
    let stored = StoredPrefs {
        font_size: Some("not-a-number".into()),
        ..StoredPrefs::default()
    };
    assert_eq!(parse_stored(stored).font_size, None);
}

#[test]
fn justify_parses_the_exact_true_and_false_tokens() {
    let true_stored = StoredPrefs {
        justify: Some("true".into()),
        ..StoredPrefs::default()
    };
    assert_eq!(parse_stored(true_stored).justify, Some(true));

    let false_stored = StoredPrefs {
        justify: Some("false".into()),
        ..StoredPrefs::default()
    };
    assert_eq!(parse_stored(false_stored).justify, Some(false));
}

#[test]
fn justify_drops_an_unrecognized_or_corrupted_token_to_none() {
    // A garbage/corrupted token must fall through to `None` — same
    // "unrecognized -> None" contract as the other prefs — rather than
    // coercing to `Some(false)` and silently overwriting the caller's
    // seeded default.
    let stored = StoredPrefs {
        justify: Some("yes".into()),
        ..StoredPrefs::default()
    };
    assert_eq!(parse_stored(stored).justify, None);
}
