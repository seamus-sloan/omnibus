//! Client-side sort helpers for the landing page.
//!
//! Pure functions over `Vec<EbookMetadata>` keyed on the user's selected
//! [`SortKey`] / [`SortDir`]. Called from [`super::LandingPage`] before
//! handing the list to the grid or table.
// The mobile build renders its own compact grid (no sort toolbar), so several
// of these web-facing helpers are dead there by design.
#![cfg_attr(feature = "mobile", allow(dead_code))]

use std::cmp::Ordering;

use omnibus_shared::{Contributor, EbookMetadata, SortDir, SortKey};

/// Join contributor names into one comma-separated display string.
pub(crate) fn contributor_names(list: &[Contributor]) -> String {
    let mut out = String::new();
    for (i, c) in list.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&c.name);
    }
    out
}

fn primary_author_key(book: &EbookMetadata) -> String {
    let c = book.creators.first();
    let name = c
        .map(|c| c.file_as.as_deref().unwrap_or(&c.name).to_string())
        .unwrap_or_default();
    name.to_ascii_lowercase()
}

fn title_key(book: &EbookMetadata) -> String {
    let t = book.title.as_deref().unwrap_or(&book.filename);
    t.to_ascii_lowercase()
}

/// Cached per-row sort key. We compute exactly one of these (matching the
/// active [`SortKey`]) per book before sorting, then `sort_by` only borrows
/// pre-built strings — no per-comparison allocation, no re-parsing of
/// `series_index`. `series_index` is normalized to milli-units of an i64 so
/// the whole struct is `Ord`-derivable (no f64 NaN issues).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RowKey {
    /// Plain string axes (Title / Author / LastUpdated / NewestAdded).
    /// `None` only for genuinely missing values; see [`cmp_with_missing_last`].
    plain: Option<String>,
    /// Series tuple: lowercased name + `series_index * 1000` rounded to i64.
    series: Option<(String, i64)>,
}

fn row_key(book: &EbookMetadata, key: SortKey) -> RowKey {
    match key {
        SortKey::Title => RowKey {
            plain: Some(title_key(book)),
            series: None,
        },
        SortKey::Author => RowKey {
            plain: Some(primary_author_key(book)),
            series: None,
        },
        SortKey::Series => RowKey {
            plain: None,
            series: book.series.as_deref().filter(|s| !s.is_empty()).map(|s| {
                let idx = book
                    .series_index
                    .as_deref()
                    .and_then(|raw| raw.parse::<f64>().ok())
                    .map(series_index_to_sort_key)
                    .unwrap_or(0);
                (s.to_ascii_lowercase(), idx)
            }),
        },
        SortKey::LastUpdated => RowKey {
            plain: book.modified.clone(),
            series: None,
        },
        SortKey::NewestAdded => RowKey {
            plain: book.added_at.clone(),
            series: None,
        },
        SortKey::RecentlyInteracted => RowKey {
            plain: book.last_interacted_at.clone(),
            series: None,
        },
    }
}

/// Pack a parsed `series_index` (`f64`) into a deterministic integer sort
/// key by scaling by 1000 (3 decimal places of precision). Guards the cast
/// so a NaN/inf parsed from a corrupt OPF can't collapse to `i64::MIN` and
/// shove the book to the top of the series sort.
fn series_index_to_sort_key(f: f64) -> i64 {
    if !f.is_finite() {
        return 0;
    }
    // Series indices in practice are small positive decimals (book 1.5 in
    // a trilogy, etc.). Cap to a sane range — well within the
    // f64-exactly-representable integer range — so the cast cannot wrap.
    const MAX_SCALED: f64 = 1.0e15;
    let scaled = (f * 1000.0).round().clamp(-MAX_SCALED, MAX_SCALED);
    #[allow(clippy::cast_possible_truncation)]
    let key = scaled as i64;
    key
}

/// Compare two `Option<K>` values where missing always sorts last regardless
/// of direction. Direction only flips ordering between two present values;
/// `None` keeps a stable "last" position so reversing a desc sort doesn't
/// shove un-timestamped or seriesless books to the top.
fn cmp_with_missing_last<K: Ord>(a: Option<&K>, b: Option<&K>, dir: SortDir) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => {
            let ord = x.cmp(y);
            if dir == SortDir::Desc {
                ord.reverse()
            } else {
                ord
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Sort a page by `key` in `dir`, with missing values last regardless of
/// direction and a never-reversed id tiebreak so equal keys stay stable.
pub(crate) fn sort_books(
    books: Vec<EbookMetadata>,
    key: SortKey,
    dir: SortDir,
) -> Vec<EbookMetadata> {
    let mut keyed: Vec<(RowKey, EbookMetadata)> =
        books.into_iter().map(|b| (row_key(&b, key), b)).collect();
    keyed.sort_by(|(ka, ba), (kb, bb)| {
        let primary = match key {
            SortKey::Series => cmp_with_missing_last(ka.series.as_ref(), kb.series.as_ref(), dir),
            _ => cmp_with_missing_last(ka.plain.as_ref(), kb.plain.as_ref(), dir),
        };
        // Stable tiebreak on id, never reversed — keeps run-to-run order
        // deterministic when the primary key matches.
        primary.then(ba.id.cmp(&bb.id))
    });
    keyed.into_iter().map(|(_, b)| b).collect()
}

/// The opposite of `d` — what the toolbar's direction button lands on.
pub(crate) fn toggle_dir(d: SortDir) -> SortDir {
    match d {
        SortDir::Asc => SortDir::Desc,
        SortDir::Desc => SortDir::Asc,
    }
}

/// The direction a freshly-picked sort key starts in — descending for the
/// two recency keys, ascending otherwise.
pub(crate) fn default_dir_for(key: SortKey) -> SortDir {
    // The recency keys feel natural with newest first.
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated | SortKey::RecentlyInteracted => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

/// The sort axes the toolbar dropdown offers, in display order.
pub(crate) const SORT_KEYS: [SortKey; 6] = [
    SortKey::Title,
    SortKey::Author,
    SortKey::Series,
    SortKey::RecentlyInteracted,
    SortKey::LastUpdated,
    SortKey::NewestAdded,
];

/// The key's wire token, as used by the dropdown, the REST query, and the
/// page cursor.
pub(crate) fn sort_key_value(key: SortKey) -> &'static str {
    // Delegate to the shared wire vocabulary so the dropdown, the REST query,
    // and the cursor axis can't drift.
    key.as_wire()
}

/// The key's human-readable dropdown label.
pub(crate) fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Title => "Title",
        SortKey::Author => "Author",
        SortKey::Series => "Series",
        SortKey::LastUpdated => "Last Updated",
        SortKey::NewestAdded => "Newest Added",
        SortKey::RecentlyInteracted => "Recently Interacted",
    }
}

/// Parse a wire token back into a sort key; `None` for anything unknown.
pub(crate) fn sort_key_from_value(value: &str) -> Option<SortKey> {
    SortKey::from_wire(value)
}

/// Stable Playwright row id derived from the ebook's on-disk filename:
/// strip directories and extension, lowercase, then collapse runs of
/// non-alphanumeric ASCII characters into a single `-` (with leading and
/// trailing dashes trimmed). The Playwright fixture table mirrors this
/// derivation so each `FIXTURE_BOOKS[i].slug` matches the row's testid.
pub(crate) fn row_slug(filename: &str) -> String {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    let stem = basename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(basename);
    let lower = stem.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_dash = true;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Stable per-book identity for a row/tile `key` and testid slug.
///
/// A fileless book (physical-only, or a ghost) has no `book_files` row, and
/// `row_to_ebook` leaves its `filename` empty — slugging that alone would
/// collapse every such book onto the same key and testid, breaking Dioxus
/// diffing and making Playwright selectors ambiguous. Fall back to the uuid,
/// which is always present and unique.
pub(crate) fn row_ident(book: &EbookMetadata) -> String {
    if book.filename.is_empty() {
        return row_slug(book.unique_identifier.as_deref().unwrap_or_default());
    }
    row_slug(&book.filename)
}

#[cfg(test)]
mod tests;
