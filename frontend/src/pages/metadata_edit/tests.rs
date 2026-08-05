//! Tests for `build_overrides`: diffing an edited form against the source
//! book to emit only the changed fields, clearing a populated field as an
//! empty string, and replacing creator/subject lists wholesale.

use super::*;

fn book_with(title: Option<&str>, creators: &[&str], subjects: &[&str]) -> EbookMetadata {
    EbookMetadata {
        title: title.map(String::from),
        creators: creators
            .iter()
            .map(|n| Contributor {
                name: (*n).to_string(),
                ..Default::default()
            })
            .collect(),
        subjects: subjects
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        ..Default::default()
    }
}

fn edited<'a>(
    title: &'a str,
    publisher: &'a str,
    authors: &'a [String],
    tags: &'a [String],
) -> EditedFields<'a> {
    EditedFields {
        title,
        description: "",
        publisher,
        published: "",
        language: "",
        series: "",
        series_index: "",
        isbn13: "",
        authors,
        tags,
        genres: &[],
    }
}

#[test]
fn build_overrides_no_changes_yields_all_none() {
    let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
    let ov = build_overrides(
        &orig,
        edited(
            "Dune",
            "",
            &["Frank Herbert".to_string()],
            &["scifi".to_string()],
        ),
    );
    assert_eq!(ov, MetadataOverrides::default());
}

#[test]
fn build_overrides_sets_only_changed_scalar_fields() {
    let orig = book_with(Some("Dune"), &["Frank Herbert"], &[]);
    let ov = build_overrides(
        &orig,
        edited("Dune: Messiah", "Ace", &["Frank Herbert".to_string()], &[]),
    );
    assert_eq!(ov.title.as_deref(), Some("Dune: Messiah"));
    assert_eq!(ov.publisher.as_deref(), Some("Ace"));
    assert!(ov.description.is_none());
    assert!(ov.creators.is_none());
    assert!(ov.subjects.is_none());
}

#[test]
fn build_overrides_clearing_a_populated_field_emits_empty_string() {
    // orig.title = "Dune", edited to "" -> the override must carry the
    // empty string so the merge clears it rather than leaving it untouched.
    let orig = book_with(Some("Dune"), &[], &[]);
    let ov = build_overrides(&orig, edited("", "", &[], &[]));
    assert_eq!(ov.title.as_deref(), Some(""));
}

#[test]
fn build_overrides_replaces_full_creator_and_subject_lists() {
    let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
    let authors = vec!["Frank Herbert".to_string(), "Brian Herbert".to_string()];
    let tags = vec!["scifi".to_string(), "classic".to_string()];
    let ov = build_overrides(&orig, edited("Dune", "", &authors, &tags));
    let creators = ov.creators.expect("creators should be set");
    assert_eq!(creators.len(), 2);
    assert_eq!(creators[0].name, "Frank Herbert");
    assert_eq!(creators[1].name, "Brian Herbert");
    assert_eq!(creators[0].role.as_deref(), Some("aut"));
    assert_eq!(
        ov.subjects,
        Some(vec!["scifi".to_string(), "classic".to_string()])
    );
}

#[test]
fn build_overrides_sets_isbn13_when_changed() {
    let orig = book_with(Some("Dune"), &[], &[]);
    let ov = build_overrides(
        &orig,
        EditedFields {
            isbn13: "9780134685991",
            ..edited("Dune", "", &[], &[])
        },
    );
    assert_eq!(ov.isbn13.as_deref(), Some("9780134685991"));
}

#[test]
fn build_overrides_clearing_a_populated_isbn13_emits_empty_string() {
    // AC4: orig.isbn13 = Some(..), edited to "" -> the override must carry
    // the empty string so the merge/apply path clears it.
    let orig = EbookMetadata {
        isbn13: Some("9780134685991".into()),
        ..book_with(Some("Dune"), &[], &[])
    };
    let ov = build_overrides(&orig, edited("Dune", "", &[], &[]));
    assert_eq!(ov.isbn13.as_deref(), Some(""));
}

#[test]
fn build_overrides_leaves_isbn13_none_when_unchanged() {
    let orig = EbookMetadata {
        isbn13: Some("9780134685991".into()),
        ..book_with(Some("Dune"), &[], &[])
    };
    let ov = build_overrides(
        &orig,
        EditedFields {
            isbn13: "9780134685991",
            ..edited("Dune", "", &[], &[])
        },
    );
    assert!(ov.isbn13.is_none());
}

#[test]
fn build_overrides_unchanged_lists_stay_none() {
    let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
    let ov = build_overrides(
        &orig,
        edited(
            "Dune",
            "",
            &["Frank Herbert".to_string()],
            &["scifi".to_string()],
        ),
    );
    assert!(ov.creators.is_none());
    assert!(ov.subjects.is_none());
}

#[test]
fn build_overrides_sets_genres_when_the_list_changed() {
    let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
    let authors = vec!["Frank Herbert".to_string()];
    let tags = vec!["scifi".to_string()];
    let genres = vec!["Science Fiction".to_string()];

    let ov = build_overrides(
        &orig,
        EditedFields {
            genres: &genres,
            ..edited("Dune", "", &authors, &tags)
        },
    );
    assert_eq!(ov.genres, Some(genres));
    // Editing genres must not also write a subjects override — that would
    // pin the scanned tag list against a later rescan.
    assert!(ov.subjects.is_none());
    assert!(ov.title.is_none());
}

#[test]
fn build_overrides_leaves_genres_none_when_the_list_is_unchanged() {
    let mut orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
    orig.genres = vec!["Science Fiction".to_string()];
    let authors = vec!["Frank Herbert".to_string()];
    let tags = vec!["scifi".to_string()];
    let genres = orig.genres.clone();

    let ov = build_overrides(
        &orig,
        EditedFields {
            genres: &genres,
            ..edited("Dune", "", &authors, &tags)
        },
    );
    assert!(ov.genres.is_none());
}

#[test]
fn build_overrides_clears_genres_with_an_empty_list() {
    // A book that had genres and now has none must send `Some([])`, not
    // `None` — `None` reads as "untouched" to the server-side merge, so a
    // `None`-based clear would silently no-op.
    let mut orig = book_with(Some("Dune"), &["Frank Herbert"], &[]);
    orig.genres = vec!["Science Fiction".to_string()];
    let authors = vec!["Frank Herbert".to_string()];

    let ov = build_overrides(
        &orig,
        EditedFields {
            genres: &[],
            ..edited("Dune", "", &authors, &[])
        },
    );
    assert_eq!(ov.genres, Some(Vec::new()));
}
