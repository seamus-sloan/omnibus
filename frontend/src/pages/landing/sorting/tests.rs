//! Tests for the landing-page sort helpers: row-slug/row-ident derivation,
//! sort-key wire round-tripping, and `sort_books` ordering across every
//! `SortKey`/`SortDir` combination, including missing-value tiebreaks.

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

// row_slug cases.
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

// contributor_names cases.
#[test]
fn contributor_names_joins_multiple_creators_with_comma_space() {
    let creators = vec![
        Contributor {
            name: "First Author".into(),
            role: None,
            file_as: None,
            id: None,
        },
        Contributor {
            name: "Second Author".into(),
            role: None,
            file_as: None,
            id: None,
        },
    ];
    assert_eq!(contributor_names(&creators), "First Author, Second Author");
}

#[test]
fn contributor_names_returns_empty_string_for_no_creators() {
    assert_eq!(contributor_names(&[]), "");
}

// toggle_dir cases.
#[test]
fn toggle_dir_flips_asc_and_desc() {
    assert_eq!(toggle_dir(SortDir::Asc), SortDir::Desc);
    assert_eq!(toggle_dir(SortDir::Desc), SortDir::Asc);
}

// sort_key_value / sort_key_label / sort_key_from_value cases.
#[test]
fn sort_key_value_delegates_to_the_shared_wire_vocabulary() {
    for key in SORT_KEYS {
        assert_eq!(sort_key_value(key), key.as_wire());
    }
}

#[test]
fn sort_key_label_names_every_sort_key() {
    assert_eq!(sort_key_label(SortKey::Title), "Title");
    assert_eq!(sort_key_label(SortKey::Author), "Author");
    assert_eq!(sort_key_label(SortKey::Series), "Series");
    assert_eq!(sort_key_label(SortKey::LastUpdated), "Last Updated");
    assert_eq!(sort_key_label(SortKey::NewestAdded), "Newest Added");
}

#[test]
fn sort_key_from_value_round_trips_every_sort_key_through_its_wire_value() {
    for key in SORT_KEYS {
        assert_eq!(sort_key_from_value(sort_key_value(key)), Some(key));
    }
}

#[test]
fn sort_key_from_value_returns_none_for_unrecognized_token() {
    assert_eq!(sort_key_from_value("not-a-real-key"), None);
}

/// A book carrying just the two fields `row_ident` reads.
fn ident_book(filename: &str, uuid: &str) -> EbookMetadata {
    EbookMetadata {
        filename: filename.into(),
        unique_identifier: Some(uuid.into()),
        ..EbookMetadata::default()
    }
}

#[test]
fn row_ident_uses_the_filename_slug_for_a_file_backed_book() {
    let b = ident_book("Alpha.epub", "11111111-2222-3333-4444-555555555555");
    assert_eq!(row_ident(&b), "alpha");
}

#[test]
fn row_ident_falls_back_to_the_uuid_for_a_fileless_book() {
    // Two physical-only books both have an empty `filename`; keying on it
    // would collide, so each must fall back to its own uuid.
    let a = ident_book("", "aaaaaaaa-0000-0000-0000-000000000000");
    let b = ident_book("", "bbbbbbbb-0000-0000-0000-000000000000");

    assert_eq!(row_ident(&a), "aaaaaaaa-0000-0000-0000-000000000000");
    assert_ne!(row_ident(&a), row_ident(&b));
}

// sort_books cases.
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
fn sort_books_by_title_asc_and_desc() {
    let s = sample();
    let asc = sort_books(s.clone(), SortKey::Title, SortDir::Asc);
    assert_eq!(ids(&asc), vec![1, 2, 3]);
    let desc = sort_books(s, SortKey::Title, SortDir::Desc);
    assert_eq!(ids(&desc), vec![3, 2, 1]);
}

#[test]
fn sort_books_by_author_asc() {
    let s = sample();
    let asc = sort_books(s, SortKey::Author, SortDir::Asc);
    // Asimov < Le Guin < Tolkien
    assert_eq!(ids(&asc), vec![2, 3, 1]);
}

#[test]
fn sort_books_by_series_grouping_with_index_then_pushes_seriesless_last() {
    let s = sample();
    let asc = sort_books(s, SortKey::Series, SortDir::Asc);
    // Foundation #1 (id 1), Foundation #2 (id 2), then no-series (id 3).
    assert_eq!(ids(&asc), vec![1, 2, 3]);
}

#[test]
fn sort_books_by_last_updated_desc_picks_most_recent_first() {
    let s = sample();
    let desc = sort_books(s, SortKey::LastUpdated, SortDir::Desc);
    // beta 2024-06 > alpha 2024-01 > gamma 2023
    assert_eq!(ids(&desc), vec![2, 1, 3]);
}

#[test]
fn sort_books_by_newest_added_desc() {
    let s = sample();
    let desc = sort_books(s, SortKey::NewestAdded, SortDir::Desc);
    // alpha 2025-03 > gamma 2025-02 > beta 2025-01
    assert_eq!(ids(&desc), vec![1, 3, 2]);
}

#[test]
fn sort_books_by_recently_interacted_desc_picks_the_latest_signal_first() {
    // Set the axis directly rather than widening `BookSpec`: the server
    // projects this field, and the client only ever reads it back.
    let mut s = sample();
    s[0].last_interacted_at = Some("2025-04-01T00:00:00".into());
    s[1].last_interacted_at = Some("2026-02-01T00:00:00".into());
    s[2].last_interacted_at = Some("2025-11-01T00:00:00".into());
    let desc = sort_books(s, SortKey::RecentlyInteracted, SortDir::Desc);
    // beta 2026-02 > gamma 2025-11 > alpha 2025-04
    assert_eq!(ids(&desc), vec![2, 3, 1]);
}

#[test]
fn sort_books_by_recently_interacted_pushes_an_untouched_book_last() {
    let mut s = sample();
    s[0].last_interacted_at = Some("2025-04-01T00:00:00".into());
    s[1].last_interacted_at = None;
    s[2].last_interacted_at = Some("2025-11-01T00:00:00".into());
    let desc = sort_books(s, SortKey::RecentlyInteracted, SortDir::Desc);
    assert_eq!(ids(&desc), vec![3, 1, 2]);
}

#[test]
fn sort_books_missing_timestamps_always_sort_last_even_on_desc() {
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
    let a = book(BookSpec {
        id: 5,
        filename: "a.epub",
        title: Some("Same"),
        authors: &[],
        series: None,
        modified: None,
        added_at: None,
        subjects: &[],
    });
    let b = book(BookSpec {
        id: 2,
        filename: "b.epub",
        title: Some("Same"),
        authors: &[],
        series: None,
        modified: None,
        added_at: None,
        subjects: &[],
    });
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
    // would `as i64` to 0/i64::MAX/i64::MIN (saturating cast), throwing the
    // book wildly out of its natural series-sort position.
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
