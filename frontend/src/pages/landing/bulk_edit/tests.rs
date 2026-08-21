use super::*;

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// A [`BulkEditValues`] with everything blank — tests override just the
/// fields they exercise.
fn empty_values<'a>() -> BulkEditValues<'a> {
    BulkEditValues {
        authors: &[],
        series: "",
        publisher: "",
        language: "",
        add_tags: &[],
        remove_tags: &[],
        add_genres: &[],
        remove_genres: &[],
    }
}

#[test]
fn build_edit_maps_blank_scalars_and_empty_authors_to_unchanged() {
    let edit = build_edit(BulkEditValues {
        series: "  ",
        language: "\t",
        ..empty_values()
    });
    assert!(edit.is_empty());
}

#[test]
fn build_edit_trims_scalars_and_carries_chip_lists() {
    let authors = strings(&["Ada Lovelace"]);
    let add_tags = strings(&["fantasy"]);
    let remove_tags = strings(&["scifi"]);
    let add_genres = strings(&["Fantasy"]);
    let remove_genres = strings(&["Sci-Fi"]);
    let edit = build_edit(BulkEditValues {
        authors: &authors,
        series: " Earthsea ",
        publisher: "Harcourt",
        language: "en",
        add_tags: &add_tags,
        remove_tags: &remove_tags,
        add_genres: &add_genres,
        remove_genres: &remove_genres,
    });
    assert_eq!(edit.authors, Some(strings(&["Ada Lovelace"])));
    assert_eq!(edit.series.as_deref(), Some("Earthsea"));
    assert_eq!(edit.publisher.as_deref(), Some("Harcourt"));
    assert_eq!(edit.language.as_deref(), Some("en"));
    assert_eq!(edit.add_tags, strings(&["fantasy"]));
    assert_eq!(edit.remove_tags, strings(&["scifi"]));
    assert_eq!(edit.add_genres, strings(&["Fantasy"]));
    assert_eq!(edit.remove_genres, strings(&["Sci-Fi"]));
}

#[test]
fn build_edit_treats_a_genre_only_edit_as_something_to_apply() {
    let add_genres = strings(&["Horror"]);
    let edit = build_edit(BulkEditValues {
        add_genres: &add_genres,
        ..empty_values()
    });
    assert!(!edit.is_empty(), "Apply must not stay disabled");
    assert!(edit.add_tags.is_empty(), "genres do not leak into tags");
}

#[test]
fn removable_genres_counts_across_books_and_ignores_tags() {
    let a = EbookMetadata {
        subjects: strings(&["scifi"]),
        genres: strings(&["Horror", "Mystery"]),
        ..Default::default()
    };
    let b = EbookMetadata {
        genres: strings(&["Horror"]),
        ..Default::default()
    };
    let named: Vec<(String, usize)> = removable_genres(&[a, b])
        .into_iter()
        .map(|i| (i.name, i.count))
        .collect();
    assert_eq!(
        named,
        vec![("Horror".to_string(), 2), ("Mystery".to_string(), 1)],
        "tags must not appear in the removable-genre pool"
    );
}

#[test]
fn removable_tags_counts_across_books_and_sorts_by_frequency_then_name() {
    let a = EbookMetadata {
        subjects: strings(&["scifi", "classic"]),
        ..Default::default()
    };
    let b = EbookMetadata {
        subjects: strings(&["scifi", "adventure"]),
        ..Default::default()
    };
    let items = removable_tags(&[a, b]);
    let named: Vec<(String, usize)> = items.into_iter().map(|i| (i.name, i.count)).collect();
    assert_eq!(
        named,
        vec![
            ("scifi".to_string(), 2),
            ("adventure".to_string(), 1),
            ("classic".to_string(), 1),
        ]
    );
}
