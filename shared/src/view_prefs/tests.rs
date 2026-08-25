//! Unit tests for library-view preference wire round-trips: the
//! `SortKey`/`SortDir` string↔enum tokens (which must stay in lockstep
//! with their serde renames) and the `ViewFilters::is_empty` predicate.

use super::*;

/// Hand-maintained list of every `SortKey` variant — keep it in sync when
/// a new variant is added, or the round-trip test below will silently skip
/// the new one. (A `SortKey::ALL` const or an exhaustive `match` in
/// `as_wire` would make this automatic; the tests just mirror the current
/// shape.)
const ALL_SORT_KEYS: [SortKey; 6] = [
    SortKey::Title,
    SortKey::Author,
    SortKey::Series,
    SortKey::LastUpdated,
    SortKey::NewestAdded,
    SortKey::RecentlyInteracted,
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
    assert_eq!(SortKey::RecentlyInteracted.as_wire(), "recently_interacted");
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

#[test]
fn index_sort_default_is_name() {
    assert_eq!(IndexSort::default(), IndexSort::Name);
}

#[test]
fn discovery_prefs_round_trips_through_json() {
    let prefs = DiscoveryPrefs {
        authors_sort: IndexSort::BookCount,
        series_sort: IndexSort::Name,
    };
    let raw = serde_json::to_string(&prefs).expect("serialize");
    assert_eq!(
        serde_json::from_str::<DiscoveryPrefs>(&raw).expect("deserialize"),
        prefs
    );
    // snake_case tokens, so a hand-written payload parses.
    assert_eq!(
        serde_json::from_str::<IndexSort>("\"book_count\"").expect("parse token"),
        IndexSort::BookCount
    );
}

#[test]
fn discovery_prefs_defaults_when_fields_absent() {
    // `#[serde(default)]` on each field keeps old/partial payloads parseable.
    let prefs: DiscoveryPrefs = serde_json::from_str("{}").expect("parse empty object");
    assert_eq!(prefs, DiscoveryPrefs::default());
}
