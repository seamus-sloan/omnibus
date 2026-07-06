//! Unit tests for library-view preference wire round-trips: the
//! `SortKey`/`SortDir` string↔enum tokens (which must stay in lockstep
//! with their serde renames) and the `ViewFilters::is_empty` predicate.

use super::*;

/// Every `SortKey` variant, so a round-trip test can't silently skip one
/// a future variant would add.
const ALL_SORT_KEYS: [SortKey; 5] = [
    SortKey::Title,
    SortKey::Author,
    SortKey::Series,
    SortKey::LastUpdated,
    SortKey::NewestAdded,
];

#[test]
fn sort_key_from_wire_round_trips_every_variant() {
    for key in ALL_SORT_KEYS {
        assert_eq!(
            SortKey::from_wire(key.as_wire()),
            Some(key),
            "as_wire/from_wire must round-trip {key:?}"
        );
    }
}

#[test]
fn sort_key_as_wire_uses_snake_case_tokens() {
    // The wire tokens are the single source of truth and are documented to
    // match the serde `snake_case` rename, so the REST `?sort=` string and
    // a serialized `SortKey` agree. Pin the exact tokens here.
    assert_eq!(SortKey::Title.as_wire(), "title");
    assert_eq!(SortKey::Author.as_wire(), "author");
    assert_eq!(SortKey::Series.as_wire(), "series");
    assert_eq!(SortKey::LastUpdated.as_wire(), "last_updated");
    assert_eq!(SortKey::NewestAdded.as_wire(), "newest_added");
}

#[test]
fn sort_key_from_wire_returns_none_for_unrecognized_token() {
    assert_eq!(SortKey::from_wire("nonsense"), None);
    assert_eq!(SortKey::from_wire(""), None);
    // Case-sensitive: the wire token is lowercase snake_case.
    assert_eq!(SortKey::from_wire("Title"), None);
    // Wrong separator: hyphen, not underscore.
    assert_eq!(SortKey::from_wire("last-updated"), None);
}

#[test]
fn sort_key_default_is_title() {
    assert_eq!(SortKey::default(), SortKey::Title);
    assert_eq!(SortKey::default().as_wire(), "title");
}

#[test]
fn sort_dir_as_wire_uses_lowercase_tokens() {
    // Documented to match the serde `lowercase` rename for the `?dir=` query.
    assert_eq!(SortDir::Asc.as_wire(), "asc");
    assert_eq!(SortDir::Desc.as_wire(), "desc");
}

#[test]
fn view_filters_is_empty_true_for_default() {
    assert!(ViewFilters::default().is_empty());
}

#[test]
fn view_filters_is_empty_false_when_any_facet_has_a_value() {
    // One case per facet: any single populated group flips the predicate.
    let with_author = ViewFilters {
        authors: vec!["Tolkien".into()],
        ..Default::default()
    };
    assert!(!with_author.is_empty());

    let with_series = ViewFilters {
        series: vec!["Poppy War".into()],
        ..Default::default()
    };
    assert!(!with_series.is_empty());

    let with_format = ViewFilters {
        formats: vec!["epub".into()],
        ..Default::default()
    };
    assert!(!with_format.is_empty());

    let with_tag = ViewFilters {
        tags: vec!["horror".into()],
        ..Default::default()
    };
    assert!(!with_tag.is_empty());
}
