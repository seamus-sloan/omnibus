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
        authors,
        tags,
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
