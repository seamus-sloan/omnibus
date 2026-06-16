//! Unit tests for the `helpers` module.

use super::*;

#[test]
fn parse_series_index_parses_finite_decimals() {
    assert_eq!(parse_series_index(" 1.5 "), Some(1.5));
    assert_eq!(parse_series_index("3"), Some(3.0));
}

#[test]
fn parse_series_index_rejects_non_finite_and_garbage() {
    // `"nan"`/`"inf"` parse to non-finite floats SQLite would store, which
    // corrupt the Series keyset cursor — they must be dropped at the source.
    assert_eq!(parse_series_index("nan"), None);
    assert_eq!(parse_series_index("inf"), None);
    assert_eq!(parse_series_index("-inf"), None);
    assert_eq!(parse_series_index("not a number"), None);
}

#[test]
fn sanitize_accent_color_accepts_indexer_shape() {
    assert_eq!(
        sanitize_accent_color(Some("oklch(0.660 0.130 245.0)")).as_deref(),
        Some("oklch(0.660 0.130 245.0)")
    );
    assert_eq!(
        sanitize_accent_color(Some("oklch(0.780 0.060 12.5)")).as_deref(),
        Some("oklch(0.780 0.060 12.5)")
    );
}

#[test]
fn sanitize_accent_color_rejects_bad_shapes() {
    for bad in [
        "",
        "red",
        "#aabbcc",
        "rgb(1,2,3)",
        "oklch(0.66, 0.13, 245)",                   // commas, not spaces
        "oklch(0.66 0.13)",                         // wrong arity
        "oklch(0.66 0.13 245 extra)",               // wrong arity
        "oklch(0.66 0.13 245",                      // missing close paren
        "0.66 0.13 245",                            // missing wrapper
        "oklch(0.66 0.13 abc)",                     // non-numeric
        "oklch(0.66 0.13 24.5.0)",                  // multiple dots
        "oklch(0.66 0.13 245); background: url(x)", // injection
        "oklch(0.66 0.13 245)\" onload=\"alert(1)", // attribute breakout
        "oklch(. . .)",                             // dot-only parts (no digits)
        "oklch(0.66 . 245.0)",                      // one part has no digits
    ] {
        assert!(
            sanitize_accent_color(Some(bad)).is_none(),
            "expected None for {bad:?}"
        );
    }
    assert_eq!(sanitize_accent_color(None), None);
}

#[test]
fn cap_query_len_passes_short_input_through_trimmed() {
    // Under the cap: only surrounding whitespace is stripped.
    assert_eq!(cap_query_len("  harry potter  "), "harry potter");
}

#[test]
fn cap_query_len_truncates_oversized_input_to_the_cap() {
    let oversized = "a".repeat(MAX_QUERY_LEN * 10);
    let capped = cap_query_len(&oversized);
    // Observable effect: the tail is dropped, leaving exactly the cap.
    assert_eq!(capped.chars().count(), MAX_QUERY_LEN);
    assert!(capped.chars().all(|c| c == 'a'));
    assert!(capped.len() < oversized.len());
}

#[test]
fn cap_query_len_truncates_on_a_char_boundary() {
    // A multibyte char repeated past the cap must slice cleanly — never
    // panic and never split a codepoint.
    let multibyte = "é".repeat(MAX_QUERY_LEN * 2);
    let capped = cap_query_len(&multibyte);
    assert_eq!(capped.chars().count(), MAX_QUERY_LEN);
    assert!(capped.chars().all(|c| c == 'é'));
}

#[test]
fn sanitize_fts_query_quotes_tokens_and_prefixes_last() {
    assert_eq!(
        sanitize_fts_query("harry pott").as_deref(),
        Some("\"harry\" \"pott\"*")
    );
}

#[test]
fn sanitize_fts_query_escapes_embedded_double_quotes() {
    assert_eq!(
        sanitize_fts_query("say \"hi").as_deref(),
        Some("\"say\" \"\"\"hi\"*")
    );
}

#[test]
fn sanitize_fts_query_returns_none_for_empty_and_whitespace() {
    assert!(sanitize_fts_query("").is_none());
    assert!(sanitize_fts_query("   \t  ").is_none());
}

#[test]
fn sanitize_fts_query_treats_operators_as_literals() {
    // Bare `AND` / `NOT` would otherwise be parsed as FTS5 operators and
    // could throw. Quoting makes them into literal tokens.
    let out = sanitize_fts_query("AND NOT OR").expect("non-empty");
    assert!(out.contains("\"AND\""));
    assert!(out.contains("\"NOT\""));
    assert!(out.contains("\"OR\"*"));
}

#[test]
fn sanitize_fts_query_keeps_hyphenated_isbn_as_single_token() {
    let out = sanitize_fts_query("978-0-123456-78-9").expect("non-empty");
    assert_eq!(out, "\"978-0-123456-78-9\"*");
}

#[test]
fn build_fts_match_returns_none_for_empty_input() {
    assert!(build_fts_match("").is_none());
    assert!(build_fts_match("   \t  ").is_none());
}

#[test]
fn build_fts_match_returns_none_when_only_empty_facets() {
    // `author:` / `series:` / `tag:` with no value are dropped silently.
    assert!(build_fts_match("author:").is_none());
    assert!(build_fts_match("series:   tag:").is_none());
}

#[test]
fn build_fts_match_emits_default_scope_for_free_text() {
    // Free-text falls into the same `{title authors series}` filter
    // that the F0.4 hardcoded filter used to apply directly.
    assert_eq!(
        build_fts_match("harry pott").as_deref(),
        Some("{title authors series} : (\"harry\" \"pott\"*)")
    );
}

#[test]
fn build_fts_match_emits_author_facet() {
    assert_eq!(
        build_fts_match("author:austen").as_deref(),
        Some("{authors} : (\"austen\"*)")
    );
}

#[test]
fn build_fts_match_combines_facet_and_free_text() {
    // Two clauses joined by an explicit `AND` — FTS5's grammar only
    // implicit-ANDs *inside* a column-filter body, not between two
    // top-level column filters.
    let out = build_fts_match("author:austen pride").expect("non-empty");
    assert_eq!(
        out,
        "{authors} : (\"austen\"*) AND {title authors series} : (\"pride\"*)"
    );
}

#[test]
fn build_fts_match_emits_series_and_tag_facets() {
    assert_eq!(
        build_fts_match("series:dune").as_deref(),
        Some("{series} : (\"dune\"*)")
    );
    assert_eq!(
        build_fts_match("tag:fiction").as_deref(),
        Some("{tags} : (\"fiction\"*)")
    );
}

#[test]
fn build_fts_match_facet_prefix_is_case_insensitive() {
    assert_eq!(
        build_fts_match("Author:Austen").as_deref(),
        Some("{authors} : (\"Austen\"*)")
    );
}

#[test]
fn build_fts_match_unknown_prefix_falls_through_to_free_text() {
    // `isbn:` is not a recognised facet — treat the whole token as
    // free-text rather than erroring.
    assert_eq!(
        build_fts_match("isbn:foo").as_deref(),
        Some("{title authors series} : (\"isbn:foo\"*)")
    );
}

#[test]
fn stable_uuid_is_deterministic() {
    // Same inputs → same UUID, both within a single run and across calls.
    let a = stable_uuid("/var/lib/omnibus", "Author/Title.epub");
    let b = stable_uuid("/var/lib/omnibus", "Author/Title.epub");
    assert_eq!(a, b, "stable_uuid must be deterministic");
}

#[test]
fn stable_uuid_differs_for_distinct_inputs() {
    // Differing library_path or filename must yield different ids; this
    // is the property that makes per-book cover URLs stable but unique.
    let base = stable_uuid("/lib", "a.epub");
    assert_ne!(base, stable_uuid("/lib", "b.epub"));
    assert_ne!(base, stable_uuid("/other", "a.epub"));
    // And the NUL separator must actually separate — splitting the key
    // at a different boundary should still produce a distinct UUID.
    assert_ne!(
        stable_uuid("/lib/a", ".epub"),
        stable_uuid("/lib", "a.epub")
    );
}

#[test]
fn stable_uuid_matches_namespace_url_v5() {
    // Cross-check against the uuid crate computing the exact same input
    // we document. Locks the namespace + key shape so a future refactor
    // can't quietly change the derivation and rotate every cover id.
    let library_path = "/var/lib/omnibus";
    let filename = "Author/Title.epub";
    let expected = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("{library_path}\0{filename}").as_bytes(),
    )
    .hyphenated()
    .to_string();
    assert_eq!(stable_uuid(library_path, filename), expected);
}

#[test]
fn stable_uuid_is_version_5() {
    // The hyphenated output must parse as a UUID with version=5 and the
    // RFC 4122 variant bits set. The pre-issue-#94 implementation set
    // neither, so this guards against regressing to a bare hex string.
    let s = stable_uuid("/lib", "x.epub");
    let parsed = uuid::Uuid::parse_str(&s).expect("stable_uuid must produce a valid UUID");
    assert_eq!(parsed.get_version_num(), 5, "must be UUIDv5");
    assert_eq!(
        parsed.get_variant(),
        uuid::Variant::RFC4122,
        "must use RFC 4122 variant bits"
    );
}

#[test]
fn format_series_index_strips_trailing_zeros_for_integer_values() {
    assert_eq!(format_series_index(1.0), "1");
    assert_eq!(format_series_index(7.0), "7");
}

#[test]
fn format_series_index_keeps_decimal_for_fractional_values() {
    assert_eq!(format_series_index(1.5), "1.5");
}

#[test]
fn format_series_index_passes_through_non_finite_values_verbatim() {
    // The guarded cast would otherwise saturate `NaN`/`inf` to
    // `i64::MIN`/`i64::MAX` and surface as a garbled integer.
    assert_eq!(format_series_index(f64::NAN), "NaN");
    assert_eq!(format_series_index(f64::INFINITY), "inf");
    assert_eq!(format_series_index(f64::NEG_INFINITY), "-inf");
}
