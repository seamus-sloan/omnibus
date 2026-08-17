//! `BulkMetadataEdit` tests: emptiness, validation, scalar-override
//! mapping to a `MetadataOverrides` layer, and tag/genre delta
//! application (`apply_tags`/`apply_genres`).

use super::super::*;
use super::tags;

#[test]
fn bulk_metadata_edit_is_empty_only_when_nothing_is_set() {
    assert!(BulkMetadataEdit::default().is_empty());
    let cases = [
        BulkMetadataEdit {
            authors: Some(vec!["A".into()]),
            ..Default::default()
        },
        BulkMetadataEdit {
            series: Some("S".into()),
            ..Default::default()
        },
        BulkMetadataEdit {
            publisher: Some("P".into()),
            ..Default::default()
        },
        BulkMetadataEdit {
            language: Some("en".into()),
            ..Default::default()
        },
        BulkMetadataEdit {
            add_tags: tags(&["t"]),
            ..Default::default()
        },
        BulkMetadataEdit {
            remove_tags: tags(&["t"]),
            ..Default::default()
        },
    ];
    for edit in cases {
        assert!(!edit.is_empty(), "expected non-empty: {edit:?}");
    }
}

#[test]
fn bulk_metadata_edit_validate_accepts_well_formed_edit() {
    let edit = BulkMetadataEdit {
        authors: Some(vec!["Ursula K. Le Guin".into()]),
        series: Some("Earthsea".into()),
        publisher: Some("Harcourt".into()),
        language: Some("en".into()),
        add_tags: tags(&["fantasy"]),
        remove_tags: tags(&["scifi"]),
        add_genres: tags(&["Fantasy"]),
        remove_genres: tags(&["Sci-Fi"]),
    };
    assert!(edit.validate().is_ok());
}

#[test]
fn bulk_metadata_edit_validate_rejects_overlong_scalar_and_tag_fields() {
    let long_name = "x".repeat(MetadataOverrides::NAME_MAX_LEN + 1);
    let long_tag = "x".repeat(MetadataOverrides::TAG_MAX_LEN + 1);
    let overlong = [
        BulkMetadataEdit {
            series: Some(long_name.clone()),
            ..Default::default()
        },
        BulkMetadataEdit {
            publisher: Some(long_name.clone()),
            ..Default::default()
        },
        BulkMetadataEdit {
            language: Some(long_name.clone()),
            ..Default::default()
        },
        BulkMetadataEdit {
            authors: Some(vec![long_name.clone()]),
            ..Default::default()
        },
        BulkMetadataEdit {
            add_tags: vec![long_tag.clone()],
            ..Default::default()
        },
        BulkMetadataEdit {
            remove_tags: vec![long_tag],
            ..Default::default()
        },
    ];
    for edit in overlong {
        assert!(edit.validate().is_err(), "expected rejection: {edit:?}");
    }
}

#[test]
fn bulk_metadata_edit_scalar_overrides_maps_authors_to_creators_and_leaves_subjects_none() {
    let edit = BulkMetadataEdit {
        authors: Some(vec!["Ada Lovelace".into(), "Grace Hopper".into()]),
        publisher: Some("Bulk House".into()),
        add_tags: tags(&["ignored-here"]),
        ..Default::default()
    };
    let overrides = edit.scalar_overrides();
    let creators = overrides.creators.expect("creators set");
    assert_eq!(creators.len(), 2);
    assert_eq!(creators[0].name, "Ada Lovelace");
    assert_eq!(creators[0].role, None);
    assert_eq!(overrides.publisher, Some("Bulk House".into()));
    assert_eq!(overrides.subjects, None);
    assert_eq!(overrides.title, None);
}

#[test]
fn bulk_metadata_edit_apply_tags_adds_missing_removes_matching_and_never_duplicates() {
    let edit = BulkMetadataEdit {
        add_tags: tags(&["fantasy", "classic"]),
        remove_tags: tags(&["scifi"]),
        ..Default::default()
    };
    let result = edit.apply_tags(&tags(&["scifi", "classic", "adventure"]));
    assert_eq!(result, tags(&["classic", "adventure", "fantasy"]));
}

#[test]
fn bulk_metadata_edit_apply_tags_add_and_remove_of_same_tag_readds_it() {
    let edit = BulkMetadataEdit {
        add_tags: tags(&["fantasy"]),
        remove_tags: tags(&["fantasy"]),
        ..Default::default()
    };
    assert_eq!(edit.apply_tags(&tags(&["fantasy"])), tags(&["fantasy"]));
    assert_eq!(edit.apply_tags(&[]), tags(&["fantasy"]));
}

#[test]
fn bulk_metadata_edit_validate_caps_each_tag_list_separately_not_combined() {
    // A large remove plus a large add is legal even though the two lists
    // together exceed MAX_SUBJECTS — they are deltas, not a subject list;
    // the per-book cap is enforced by the db merge on the true result.
    let forty: Vec<String> = (0..40).map(|i| format!("tag{i}")).collect();
    let edit = BulkMetadataEdit {
        add_tags: forty.clone(),
        remove_tags: forty,
        ..Default::default()
    };
    assert!(edit.validate().is_ok());

    let oversized: Vec<String> = (0..MetadataOverrides::MAX_SUBJECTS + 1)
        .map(|i| format!("tag{i}"))
        .collect();
    let edit = BulkMetadataEdit {
        add_tags: oversized,
        ..Default::default()
    };
    assert!(edit.validate().is_err());
}

// --- BulkMetadataEdit genre deltas ---------------------------------------

#[test]
fn bulk_metadata_edit_apply_genres_adds_missing_removes_matching_and_never_duplicates() {
    let edit = BulkMetadataEdit {
        add_genres: tags(&["Fantasy", "Mystery"]),
        remove_genres: tags(&["Horror"]),
        ..Default::default()
    };
    let result = edit.apply_genres(&tags(&["Horror", "Mystery", "Romance"]));
    assert_eq!(result, tags(&["Mystery", "Romance", "Fantasy"]));
}

#[test]
fn bulk_metadata_edit_is_empty_is_false_when_only_genre_deltas_are_set() {
    let edit = BulkMetadataEdit {
        add_genres: tags(&["Fantasy"]),
        ..Default::default()
    };
    assert!(!edit.is_empty());
    assert!(BulkMetadataEdit::default().is_empty());
}

#[test]
fn bulk_metadata_edit_validate_caps_each_genre_list_separately_not_combined() {
    let forty: Vec<String> = (0..40).map(|i| format!("genre{i}")).collect();
    let edit = BulkMetadataEdit {
        add_genres: forty.clone(),
        remove_genres: forty,
        ..Default::default()
    };
    assert!(edit.validate().is_ok());

    let oversized: Vec<String> = (0..MetadataOverrides::MAX_GENRES + 1)
        .map(|i| format!("genre{i}"))
        .collect();
    let edit = BulkMetadataEdit {
        add_genres: oversized,
        ..Default::default()
    };
    assert!(edit.validate().is_err());
}
