//! Pure, dependency-free helpers shared across the db query layer:
//! deterministic UUID derivation, filename / accent-color sanitisation,
//! and the FTS5 query / match builders.

use std::path::Path;

/// Maximum query length (in chars) accepted by the FTS5 search entrypoints
/// (`search_books`, `count_search_books`, `search_palette`). Inputs beyond
/// this are truncated before reaching [`build_fts_match`] / `LIKE` so the
/// generated MATCH expression and pattern length stay bounded regardless of
/// caller payload size (issue #189). Module-wide so the limit is tunable in
/// one place across every search path.
pub(crate) const MAX_QUERY_LEN: usize = 256;

/// Deterministic UUIDv5 derived from `(library_path, filename)` so reindexing
/// the same file produces the same uuid. Keeps `/api/covers/{uuid}` URLs
/// stable across reindex cycles even as the primary `books.id` renumbers.
///
/// Implemented as `Uuid::new_v5(NAMESPACE_URL, "{library_path}\0{filename}")`.
/// Issue #94: the previous implementation used
/// `std::collections::hash_map::DefaultHasher`, whose algorithm is documented
/// as subject to change between Rust toolchain versions. A toolchain bump
/// would silently rotate every cover UUID on the next reindex and orphan
/// every cover file on disk. UUIDv5 (SHA-1 over a namespace + name, per
/// RFC 4122 §4.3) is fixed across toolchains, sets the proper version/variant
/// bits, and emits the canonical 8-4-4-4-12 hyphenated form.
pub(crate) fn stable_uuid(library_path: &str, filename: &str) -> String {
    // NUL is the one byte that can't appear inside either side, so it's a
    // safe, unambiguous separator — no `(library_path, filename)` collision
    // is reachable from a different split.
    let key = format!("{library_path}\0{filename}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, key.as_bytes())
        .hyphenated()
        .to_string()
}

/// Split `dir/sub/name.epub` into (`dir/sub`, `name`, `EPUB`). If no dir,
/// the path portion is empty. Extension is uppercased per Calibre convention.
pub(crate) fn split_filename(filename: &str) -> (String, String, String) {
    let path = Path::new(filename);
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    (parent, stem, ext)
}

pub(crate) fn parse_series_index(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Defense-in-depth gate on `books.accent_color` (#125). The indexer's
/// `extract_accent` emits strings of the exact shape `oklch(L C H)` with
/// three space-separated decimal floats; consumers (Atrium cover tiles,
/// palette rows, book detail) inline the value into an HTML `style`
/// attribute. Reject anything that doesn't match that strict shape so a
/// future override path or imported value can't smuggle CSS / break out
/// of the attribute, regardless of consumer escaping.
pub(crate) fn sanitize_accent_color(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    let inner = s.strip_prefix("oklch(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(' ').collect();
    if parts.len() != 3 {
        return None;
    }
    for part in &parts {
        if part.is_empty() || part.matches('.').count() > 1 {
            return None;
        }
        let mut has_digit = false;
        for c in part.chars() {
            match c {
                '0'..='9' => has_digit = true,
                '.' => {}
                _ => return None,
            }
        }
        // Reject parts with no digits at all (a bare "." or ".." stripped of
        // characters). The `extract_accent` formatter always emits at least
        // one digit on each side per `{l:.3}` / `{c:.3}` / `{h:.1}`.
        if !has_digit {
            return None;
        }
    }
    Some(s.to_string())
}

/// Join an iterator of names into a single whitespace-separated string for
/// the FTS `authors` / `tags` columns. Empty inputs collapse to "".
pub(crate) fn join_names<'a, I: IntoIterator<Item = &'a str>>(iter: I) -> String {
    let mut out = String::new();
    for name in iter {
        if name.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(name);
    }
    out
}

/// Parse a user-typed query into a single FTS5 MATCH expression.
///
/// Recognises `author:foo`, `series:foo`, `tag:foo` (case-insensitive on
/// the prefix) and emits column-scoped clauses. Everything else falls
/// through to the default `{title authors series}` filter as free-text
/// terms, preserving the existing scope and prefix-on-last semantics from
/// [`sanitize_fts_query`].
///
/// Returns `None` when nothing usable remains (empty input, or only empty
/// `author:` / `series:` / `tag:` tokens) so callers can short-circuit
/// instead of submitting an empty `MATCH`.
pub fn build_fts_match(raw: &str) -> Option<String> {
    let mut author_tokens: Vec<&str> = Vec::new();
    let mut series_tokens: Vec<&str> = Vec::new();
    let mut tag_tokens: Vec<&str> = Vec::new();
    let mut free_tokens: Vec<&str> = Vec::new();

    for token in raw.split_whitespace() {
        if let Some((prefix, value)) = token.split_once(':') {
            let lower = prefix.to_ascii_lowercase();
            if value.is_empty() {
                // `author:` with no value — drop silently rather than
                // treating it as free-text or erroring.
                if matches!(lower.as_str(), "author" | "series" | "tag") {
                    continue;
                }
            }
            match lower.as_str() {
                "author" => {
                    author_tokens.push(value);
                    continue;
                }
                "series" => {
                    series_tokens.push(value);
                    continue;
                }
                "tag" => {
                    tag_tokens.push(value);
                    continue;
                }
                _ => {}
            }
        }
        free_tokens.push(token);
    }

    let mut clauses: Vec<String> = Vec::new();
    if let Some(s) = sanitize_fts_tokens(&author_tokens) {
        clauses.push(format!("{{authors}} : ({s})"));
    }
    if let Some(s) = sanitize_fts_tokens(&series_tokens) {
        clauses.push(format!("{{series}} : ({s})"));
    }
    if let Some(s) = sanitize_fts_tokens(&tag_tokens) {
        clauses.push(format!("{{tags}} : ({s})"));
    }
    if let Some(s) = sanitize_fts_tokens(&free_tokens) {
        // Default scope: title/authors/series (matches F0.4 design — keeps
        // short prefix queries from dragging in generic tag/description
        // values).
        clauses.push(format!("{{title authors series}} : ({s})"));
    }

    if clauses.is_empty() {
        None
    } else {
        // Multiple column-filter clauses must be joined with an explicit
        // FTS5 boolean operator — implicit AND only works *inside* a
        // single column filter's `( ... )` body.
        Some(clauses.join(" AND "))
    }
}

/// Wrap each whitespace-separated token in double-quotes and append `*` to
/// the last one for prefix matching. This lets the user type plain words
/// (including FTS5-reserved tokens like `AND`/`NOT` or hyphenated ISBNs)
/// without triggering a `MATCH` parse error, and gives type-ahead cheaply.
///
/// Returns `None` when the sanitized query is empty — callers should treat
/// that as "don't run the query" rather than passing an empty MATCH.
pub fn sanitize_fts_query(raw: &str) -> Option<String> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    sanitize_fts_tokens(&tokens)
}

/// Per-token quoting/escaping shared by [`sanitize_fts_query`] and
/// [`build_fts_match`]. Tokens are assumed to be whitespace-free (the
/// callers split on whitespace first).
fn sanitize_fts_tokens(tokens: &[&str]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for token in tokens {
        // Double quotes inside a token would terminate the quoted phrase.
        // FTS5's quoted phrase escape is `""`, so double every quote.
        let escaped = token.replace('"', "\"\"");
        if escaped.is_empty() {
            continue;
        }
        parts.push(format!("\"{escaped}\""));
    }
    if parts.is_empty() {
        return None;
    }
    let last = parts.len() - 1;
    parts[last].push('*');
    Some(parts.join(" "))
}

pub(crate) fn format_series_index(v: f64) -> String {
    if (v - v.trunc()).abs() < f64::EPSILON {
        format!("{}", v.trunc() as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- accent_color sanitiser ----------

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

    // ---------- FTS5 (F0.4) ----------

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

    // ---------- stable_uuid (Issue #94) ----------

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
}
