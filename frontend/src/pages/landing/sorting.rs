//! Client-side sort helpers for the landing page.
//!
//! Pure functions over `Vec<EbookMetadata>` keyed on the user's selected
//! [`SortKey`] / [`SortDir`]. Called from [`super::LandingPage`] before
//! handing the list to the grid or table.

use std::cmp::Ordering;

use omnibus_shared::{Contributor, EbookMetadata, SortDir, SortKey};

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

pub(crate) fn toggle_dir(d: SortDir) -> SortDir {
    match d {
        SortDir::Asc => SortDir::Desc,
        SortDir::Desc => SortDir::Asc,
    }
}

pub(crate) fn default_dir_for(key: SortKey) -> SortDir {
    // "Newest Added" / "Last Updated" feel natural with newest first.
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

pub(crate) const SORT_KEYS: [SortKey; 5] = [
    SortKey::Title,
    SortKey::Author,
    SortKey::Series,
    SortKey::LastUpdated,
    SortKey::NewestAdded,
];

pub(crate) fn sort_key_value(key: SortKey) -> &'static str {
    match key {
        SortKey::Title => "title",
        SortKey::Author => "author",
        SortKey::Series => "series",
        SortKey::LastUpdated => "last_updated",
        SortKey::NewestAdded => "newest_added",
    }
}

pub(crate) fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Title => "Title",
        SortKey::Author => "Author",
        SortKey::Series => "Series",
        SortKey::LastUpdated => "Last Updated",
        SortKey::NewestAdded => "Newest Added",
    }
}

pub(crate) fn sort_key_from_value(value: &str) -> Option<SortKey> {
    match value {
        "title" => Some(SortKey::Title),
        "author" => Some(SortKey::Author),
        "series" => Some(SortKey::Series),
        "last_updated" => Some(SortKey::LastUpdated),
        "newest_added" => Some(SortKey::NewestAdded),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_shared::Contributor;

    struct BookSpec<'a> {
        id: i64,
        filename: &'a str,
        title: Option<&'a str>,
        authors: &'a [(&'a str, Option<&'a str>)],
        series: Option<(&'a str, &'a str)>,
        modified: Option<&'a str>,
        added_at: Option<&'a str>,
        subjects: &'a [&'a str],
    }

    fn book(s: BookSpec<'_>) -> EbookMetadata {
        EbookMetadata {
            id: s.id,
            filename: s.filename.into(),
            title: s.title.map(Into::into),
            creators: s
                .authors
                .iter()
                .map(|(name, file_as)| Contributor {
                    name: (*name).into(),
                    role: None,
                    file_as: file_as.map(Into::into),
                    id: None,
                })
                .collect(),
            series: s.series.map(|(name, _)| name.into()),
            series_index: s.series.map(|(_, idx)| idx.into()),
            modified: s.modified.map(Into::into),
            added_at: s.added_at.map(Into::into),
            subjects: s.subjects.iter().map(|sub| (*sub).to_string()).collect(),
            ..Default::default()
        }
    }

    fn ids(books: &[EbookMetadata]) -> Vec<i64> {
        books.iter().map(|b| b.id).collect()
    }

    // --- row_slug ---

    #[test]
    fn row_slug_lowercases_and_strips_extension() {
        assert_eq!(row_slug("Alpha.epub"), "alpha");
    }
    #[test]
    fn row_slug_collapses_runs_of_non_alphanumerics() {
        assert_eq!(row_slug("Beta in the Series.epub"), "beta-in-the-series");
    }
    #[test]
    fn row_slug_uses_basename_for_nested_paths() {
        assert_eq!(row_slug("series/vol1/Deep Book.epub"), "deep-book");
    }
    #[test]
    fn row_slug_trims_trailing_dashes() {
        assert_eq!(row_slug("weird---name!!!.epub"), "weird-name");
    }
    #[test]
    fn row_slug_handles_filename_without_extension() {
        assert_eq!(row_slug("plain"), "plain");
    }

    // --- sort_books ---

    fn sample() -> Vec<EbookMetadata> {
        vec![
            book(BookSpec {
                id: 1,
                filename: "alpha.epub",
                title: Some("Alpha"),
                authors: &[("Tolkien, J.R.R.", Some("Tolkien, J.R.R."))],
                series: Some(("Foundation", "1")),
                modified: Some("2024-01-01T00:00:00"),
                added_at: Some("2025-03-10T00:00:00"),
                subjects: &["Fantasy"],
            }),
            book(BookSpec {
                id: 2,
                filename: "beta.epub",
                title: Some("Beta"),
                authors: &[("Asimov, Isaac", Some("Asimov, Isaac"))],
                series: Some(("Foundation", "2")),
                modified: Some("2024-06-01T00:00:00"),
                added_at: Some("2025-01-05T00:00:00"),
                subjects: &["Sci-Fi"],
            }),
            book(BookSpec {
                id: 3,
                filename: "gamma.epub",
                title: Some("Gamma"),
                authors: &[("Le Guin, Ursula", Some("Le Guin, Ursula"))],
                series: None,
                modified: Some("2023-01-01T00:00:00"),
                added_at: Some("2025-02-20T00:00:00"),
                subjects: &["Fantasy", "Sci-Fi"],
            }),
        ]
    }

    #[test]
    fn sorts_by_title_asc_and_desc() {
        let s = sample();
        let asc = sort_books(s.clone(), SortKey::Title, SortDir::Asc);
        assert_eq!(ids(&asc), vec![1, 2, 3]);
        let desc = sort_books(s, SortKey::Title, SortDir::Desc);
        assert_eq!(ids(&desc), vec![3, 2, 1]);
    }

    #[test]
    fn sorts_by_author_asc() {
        let s = sample();
        let asc = sort_books(s, SortKey::Author, SortDir::Asc);
        // Asimov < Le Guin < Tolkien
        assert_eq!(ids(&asc), vec![2, 3, 1]);
    }

    #[test]
    fn sorts_by_series_grouping_with_index_then_pushes_seriesless_last() {
        let s = sample();
        let asc = sort_books(s, SortKey::Series, SortDir::Asc);
        // Foundation #1 (id 1), Foundation #2 (id 2), then no-series (id 3).
        assert_eq!(ids(&asc), vec![1, 2, 3]);
    }

    #[test]
    fn sorts_by_last_updated_desc_picks_most_recent_first() {
        let s = sample();
        let desc = sort_books(s, SortKey::LastUpdated, SortDir::Desc);
        // beta 2024-06 > alpha 2024-01 > gamma 2023
        assert_eq!(ids(&desc), vec![2, 1, 3]);
    }

    #[test]
    fn sorts_by_newest_added_desc() {
        let s = sample();
        let desc = sort_books(s, SortKey::NewestAdded, SortDir::Desc);
        // alpha 2025-03 > gamma 2025-02 > beta 2025-01
        assert_eq!(ids(&desc), vec![1, 3, 2]);
    }

    #[test]
    fn missing_timestamps_always_sort_last_even_on_desc() {
        // Two timestamped books + one with no `modified` value. In descending
        // order the most-recent timestamp comes first, but the missing-value
        // book stays at the end (it doesn't get flipped to the top by the
        // direction reversal).
        let books = vec![
            book(BookSpec {
                id: 1,
                filename: "old.epub",
                title: Some("Old"),
                authors: &[],
                series: None,
                modified: Some("2024-01-01T00:00:00"),
                added_at: None,
                subjects: &[],
            }),
            book(BookSpec {
                id: 2,
                filename: "new.epub",
                title: Some("New"),
                authors: &[],
                series: None,
                modified: Some("2025-01-01T00:00:00"),
                added_at: None,
                subjects: &[],
            }),
            book(BookSpec {
                id: 3,
                filename: "missing.epub",
                title: Some("Missing"),
                authors: &[],
                series: None,
                modified: None,
                added_at: None,
                subjects: &[],
            }),
        ];
        let desc = sort_books(books.clone(), SortKey::LastUpdated, SortDir::Desc);
        assert_eq!(ids(&desc), vec![2, 1, 3]);
        let asc = sort_books(books, SortKey::LastUpdated, SortDir::Asc);
        assert_eq!(ids(&asc), vec![1, 2, 3]);
    }

    #[test]
    fn series_sort_keeps_seriesless_last_in_desc_too() {
        let s = sample();
        let desc = sort_books(s, SortKey::Series, SortDir::Desc);
        // Foundation #2 (id 2) → Foundation #1 (id 1) → seriesless gamma (id 3)
        // last regardless of direction.
        assert_eq!(ids(&desc), vec![2, 1, 3]);
    }

    #[test]
    fn sort_by_title_is_stable_on_equal_keys_via_id_tiebreak() {
        // Two books share the same title; the id tiebreak keeps a deterministic
        // order regardless of input order or sort direction.
        let mut a = book(BookSpec {
            id: 5,
            filename: "a.epub",
            title: Some("Same"),
            authors: &[],
            series: None,
            modified: None,
            added_at: None,
            subjects: &[],
        });
        let mut b = book(BookSpec {
            id: 2,
            filename: "b.epub",
            title: Some("Same"),
            authors: &[],
            series: None,
            modified: None,
            added_at: None,
            subjects: &[],
        });
        a.title = Some("Same".into());
        b.title = Some("Same".into());
        let asc = sort_books(vec![a.clone(), b.clone()], SortKey::Title, SortDir::Asc);
        assert_eq!(ids(&asc), vec![2, 5]);
        // Reversed direction does not reverse the tiebreak: ids stay ascending.
        let desc = sort_books(vec![a, b], SortKey::Title, SortDir::Desc);
        assert_eq!(ids(&desc), vec![2, 5]);
    }

    #[test]
    fn series_index_to_sort_key_scales_finite_values_by_one_thousand() {
        assert_eq!(series_index_to_sort_key(1.0), 1_000);
        assert_eq!(series_index_to_sort_key(1.5), 1_500);
        assert_eq!(series_index_to_sort_key(0.0), 0);
        assert_eq!(series_index_to_sort_key(-2.0), -2_000);
    }

    #[test]
    fn series_index_to_sort_key_returns_zero_for_non_finite_input() {
        // The bug this guards against: a NaN/inf parsed from a corrupt OPF
        // would `as i64` to `i64::MIN`, shoving the book to the top of the
        // series sort.
        assert_eq!(series_index_to_sort_key(f64::NAN), 0);
        assert_eq!(series_index_to_sort_key(f64::INFINITY), 0);
        assert_eq!(series_index_to_sort_key(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn series_index_to_sort_key_clamps_extreme_finite_values_in_range() {
        let huge = series_index_to_sort_key(1.0e30);
        let tiny = series_index_to_sort_key(-1.0e30);
        // Both fall in the f64-exact range so the cast can't saturate to
        // the wrong sign.
        assert!(huge > 0);
        assert!(tiny < 0);
    }
}
