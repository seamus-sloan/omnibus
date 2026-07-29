//! Tests for the grid quick-edit save payload construction.

use super::*;

#[test]
fn field_override_sends_empty_string_sentinel_when_series_is_cleared() {
    // `EditableCell` only calls the save handler once the value has
    // actually changed, so an empty input here means "the user cleared
    // this field" — the payload must carry `Some("")` (the clear
    // sentinel `merge_metadata_overrides` understands), not `None`
    // (which reads as "untouched" and silently keeps whatever override
    // already exists — the #1085 bug for AC2).
    let ov = field_override(EditField::Series, "");
    assert_eq!(ov.series, Some(String::new()));
}

#[test]
fn field_override_sends_empty_string_sentinel_when_language_is_cleared() {
    // Same contract as the series case, covering AC3.
    let ov = field_override(EditField::Language, "  ");
    assert_eq!(ov.language, Some(String::new()));
}

#[test]
fn field_override_trims_and_sets_value_when_field_is_edited() {
    let ov = field_override(EditField::Title, "  New Title  ");
    assert_eq!(ov.title, Some("New Title".to_string()));
    // Untouched scalar fields stay `None` so a merge preserves them.
    assert_eq!(ov.series, None);
    assert_eq!(ov.publisher, None);
}

#[test]
fn field_override_only_sets_the_targeted_scalar_field() {
    let ov = field_override(EditField::Published, "2024");
    assert_eq!(ov.published, Some("2024".to_string()));
    assert_eq!(ov.title, None);
    assert_eq!(ov.language, None);
}

#[test]
fn field_override_is_noop_for_authors_field() {
    // Authors edits are routed through `build_save_authors`/`creators`
    // instead — `build_save_field` returns early for this variant, but
    // `field_override` itself must also stay a no-op if ever called
    // directly with it.
    let ov = field_override(EditField::Authors, "Someone");
    assert_eq!(ov, MetadataOverrides::default());
}

#[test]
fn field_override_is_noop_for_tags_field() {
    // Same contract as the authors case: tags edits go through
    // `build_save_tags`/`subjects`, never the scalar path.
    let ov = field_override(EditField::Tags, "fiction");
    assert_eq!(ov, MetadataOverrides::default());
}
