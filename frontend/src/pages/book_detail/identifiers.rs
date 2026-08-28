//! The book's identifier rows as the detail page's metadata table shows
//! them: a human label for each scheme (an ONIX codelist-5 code is not one),
//! and one row per distinct value. Shared by the marquee and mobile tables.

use omnibus_shared::Identifier;

/// Collision-free list key for an identifier row. A book can carry several
/// identifiers sharing one `scheme` (the projection keeps every distinct
/// value per scheme), so the key folds in `value` to stay unique among the
/// keyed siblings — Dioxus panics when two keyed siblings share a key.
///
/// Both fields are `Debug`-quoted (not joined with a plain separator): a raw
/// `scheme|value` join collides when the data itself contains the delimiter
/// (`scheme="a|b", value="c"` vs `scheme="a", value="b|c"`), which would
/// reintroduce the very panic this guards against. `Debug` escapes embedded
/// quotes/backslashes, so the `(scheme, value)` pair maps injectively to the
/// key.
pub(super) fn bd_identifier_key(ident: &Identifier) -> String {
    format!("{:?}\u{1f}{:?}", ident.scheme, ident.value)
}

/// How confidently a row's label names what it holds. Two identifiers can
/// carry the same value under different schemes — an EPUB 3 package writes
/// its ISBN once as `<dc:identifier>` and again as an ONIX codelist-5
/// refinement — and [`bd_identifier_rows`] keeps the best-named of them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LabelRank {
    /// Inferred from the value's shape; the scheme said nothing usable.
    Inferred,
    /// The source's own scheme, passed through as written.
    RawScheme,
    /// A scheme this table recognizes and can name properly.
    Known,
}

/// Human label for a scheme this table recognizes. Covers the ONIX
/// codelist-5 numbers an EPUB 3 `identifier-type` refinement carries (a
/// reader should never be shown the bare code `15`) and the scheme names
/// Calibre and the retailers write.
///
/// `uuid` is deliberately not "UUID": the value is the *source file's* uuid,
/// which is not the book's own uuid — the one Omnibus keys everything on and
/// shows in the URL.
fn bd_known_scheme_label(scheme: &str) -> Option<&'static str> {
    let label = match scheme.trim().to_ascii_lowercase().as_str() {
        "01" | "proprietary" => "Proprietary ID",
        "02" | "isbn-10" | "isbn10" => "ISBN-10",
        "03" | "gtin-13" | "ean" | "ean-13" => "EAN-13",
        "04" | "upc" => "UPC",
        "05" | "ismn-10" => "ISMN-10",
        "06" | "doi" => "DOI",
        "13" | "lccn" => "LCCN",
        "14" | "gtin-14" => "GTIN-14",
        "15" | "isbn-13" | "isbn13" => "ISBN-13",
        "17" | "isbn-a" => "ISBN-A",
        "22" | "urn" => "URN",
        "23" | "oclc" => "OCLC",
        "24" | "url" | "uri" => "URL",
        "25" | "ismn-13" => "ISMN-13",
        "isbn" => "ISBN",
        "asin" | "amazon" | "mobi-asin" => "ASIN",
        "google" => "Google Books ID",
        "goodreads" => "Goodreads ID",
        "calibre" => "Calibre ID",
        "uuid" => "Source UUID",
        _ => return None,
    };
    Some(label)
}

/// Label for a file-details identifier row, plus how well the source named
/// it. A recognized scheme gets a human label; an unrecognized *named* scheme
/// passes through as written; a missing, `unknown`, or bare-numeric scheme
/// falls back to the value's own shape, so no row is ever labelled with a raw
/// codelist number.
fn bd_identifier_label_ranked(ident: &Identifier) -> (String, LabelRank) {
    match ident.scheme.as_deref().map(str::trim) {
        Some(scheme) if bd_known_scheme_label(scheme).is_some() => (
            bd_known_scheme_label(scheme).unwrap_or(scheme).to_string(),
            LabelRank::Known,
        ),
        Some(scheme)
            if !scheme.is_empty()
                && !scheme.eq_ignore_ascii_case("unknown")
                // An unrecognized all-digit scheme is a codelist value this
                // table doesn't know, not a name worth showing a reader.
                && !scheme.chars().all(|c| c.is_ascii_digit()) =>
        {
            (scheme.to_string(), LabelRank::RawScheme)
        }
        _ if bd_looks_like_isbn(&ident.value) => ("ISBN".to_string(), LabelRank::Inferred),
        _ => ("Identifier".to_string(), LabelRank::Inferred),
    }
}

/// One row of the book-detail identifier table.
pub(super) struct BdIdentifierRow {
    /// Dioxus list key, from the identifier the row's label came from.
    pub key: String,
    pub label: String,
    pub value: String,
}

/// The identifier rows to render, deduplicated by value.
///
/// A book routinely carries one identifier under several schemes — an EPUB 3
/// package repeats its ISBN as an ONIX refinement, and a merge folds two
/// editions' identifier sets together — which listed one value on as many
/// rows as it had schemes. Two identifiers with the same value *are* the same
/// identifier, so the rows collapse to one, keeping the best-named label
/// (which also subsumes the `(scheme, value)` dedup the DB's primary key
/// already gives us) and the first occurrence's position.
pub(super) fn bd_identifier_rows(identifiers: &[Identifier]) -> Vec<BdIdentifierRow> {
    let mut rows: Vec<(BdIdentifierRow, LabelRank)> = Vec::new();
    for ident in identifiers {
        let value = ident.value.trim();
        if value.is_empty() {
            continue;
        }
        let (label, rank) = bd_identifier_label_ranked(ident);
        let row = BdIdentifierRow {
            key: bd_identifier_key(ident),
            label,
            value: value.to_string(),
        };
        match rows
            .iter_mut()
            .find(|(existing, _)| existing.value.eq_ignore_ascii_case(value))
        {
            // Strictly better only: ties keep the first occurrence, so the
            // order the projection emits stays the order a reader sees.
            Some(slot) if rank > slot.1 => *slot = (row, rank),
            Some(_) => {}
            None => rows.push((row, rank)),
        }
    }
    rows.into_iter().map(|(row, _)| row).collect()
}

/// True when `value`, with hyphens and whitespace stripped, is the right length for an ISBN-10 or ISBN-13, digits-only except a trailing ISBN-10 `X` check digit.
fn bd_looks_like_isbn(value: &str) -> bool {
    let cleaned: Vec<char> = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    match cleaned.len() {
        10 => {
            let last = cleaned.len() - 1;
            cleaned
                .iter()
                .enumerate()
                .all(|(i, c)| c.is_ascii_digit() || (i == last && c.eq_ignore_ascii_case(&'x')))
        }
        13 => cleaned.iter().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
