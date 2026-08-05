//! Pure helpers shared across the db query layer:
//! deterministic UUID derivation, filename / accent-color sanitisation,
//! and the FTS5 query / match builders.

use std::path::Path;

/// Maximum query length (in chars) accepted by the FTS5 search entrypoints
/// (`search_books`, `count_search_books`, `search_palette`). Inputs beyond
/// this are truncated before reaching [`build_fts_match`] / `LIKE` so the
/// generated MATCH expression and pattern length stay bounded regardless of
/// caller payload size. Module-wide so the limit is tunable in one place
/// across every search path.
pub(crate) const MAX_QUERY_LEN: usize = 256;

/// Trim surrounding whitespace and cap a user query to [`MAX_QUERY_LEN`]
/// chars before it reaches [`build_fts_match`] / `LIKE`. Collecting chars
/// (not bytes) guarantees the truncation never lands mid-codepoint. Shared
/// by every search entrypoint so the cap is applied identically everywhere.
pub(crate) fn cap_query_len(q: &str) -> String {
    q.trim().chars().take(MAX_QUERY_LEN).collect()
}

/// Encode library paths for binding through SQLite's `json_each`.
pub(crate) fn library_paths_json(library_paths: &[&str]) -> String {
    serde_json::Value::Array(
        library_paths
            .iter()
            .map(|path| serde_json::Value::String((*path).to_string()))
            .collect(),
    )
    .to_string()
}

/// Deterministic UUIDv5 derived from `(library_path, filename)` so reindexing
/// the same file produces the same uuid. Keeps `/api/covers/{uuid}` URLs
/// stable across reindex cycles even as the primary `books.id` renumbers.
/// UUIDv5 (SHA-1 over a namespace + name, per RFC 4122 §4.3) is fixed
/// across Rust toolchains — a previous `DefaultHasher` derivation rotated
/// every cover UUID on toolchain bumps and orphaned cover files on disk.
pub(crate) fn stable_uuid(library_path: &str, filename: &str) -> String {
    // NUL is the one byte that can't appear inside either side, so it's a
    // safe, unambiguous separator — no `(library_path, filename)` collision
    // is reachable from a different split.
    let key = format!("{library_path}\0{filename}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, key.as_bytes())
        .hyphenated()
        .to_string()
}

/// Mint a fresh, durable book identity — a random UUIDv4 assigned once at
/// insert and **never recomputed** (F2). Unlike the retired path-derived
/// `stable_uuid`, this cannot move when a library root is repointed and
/// cannot collide for two distinct books that share a title/author or even
/// identical bytes. The diff no longer reconstructs this value from disk; the
/// relative-path [`scan_key_for`] does the Phase-A matching instead.
pub(crate) fn mint_uuid() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The Phase-A diff key: a book's path *relative to its scan root* (e.g.
/// `Author/Title.epub`). Stored in `books.scan_key` and matched disk-vs-DB so
/// a library-root repoint — which leaves relative paths unchanged — preserves
/// every `books.uuid` instead of re-minting it. Identity today (the relative
/// path is used verbatim); the single chokepoint should we ever need to fold
/// separators or casing.
pub(crate) fn scan_key_for(relative_path: &str) -> String {
    relative_path.to_string()
}

/// Dot-directories and Synology's `@eaDir` — never library content, and
/// routinely unreadable, which would flag the walk `incomplete` and suppress
/// the removal pass on every scan (issue #819).
pub(crate) fn is_skipped_scan_dir(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("@eaDir")
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

/// Parse a series index, rejecting non-finite values. `"nan"`/`"inf"` parse to
/// floats SQLite stores happily, but a non-finite `series_index` corrupts the
/// Series-axis ordering and breaks the keyset cursor (`serde_json` refuses to
/// encode NaN/Inf), so drop them here at the source.
pub(crate) fn parse_series_index(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Defense-in-depth gate on `books.accent_color`. The indexer's
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
    if !v.is_finite() {
        return format!("{v}");
    }
    if (v - v.trunc()).abs() < f64::EPSILON {
        // Clamp to the f64-exactly-representable integer range before the
        // cast so an out-of-range value can't saturate to `i64::MIN`/MAX
        // and surface as `-9223372036854775808` in the UI.
        const LIMIT: f64 = 9.0e15;
        let bounded = v.trunc().clamp(-LIMIT, LIMIT);
        #[allow(clippy::cast_possible_truncation)]
        let i = bounded as i64;
        format!("{i}")
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests;
