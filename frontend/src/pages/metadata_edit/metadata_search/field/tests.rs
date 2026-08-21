//! Coverage for the field table. The completeness tests are the ones that
//! matter: they are what keeps "add a field" from quietly meaning "add a
//! field and remember four other places".

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::MetadataProvider;

use super::*;

/// A source that answers for every field, so a test can tell "this field has
/// no accessor" apart from "this provider has no value".
fn full_edition() -> ProviderEdition {
    ProviderEdition {
        source: MetadataProvider::GoogleBooks,
        provider_ref: "gb-1".into(),
        isbn13: Some("9780134685991".into()),
        isbn10: Some("0134685997".into()),
        title: "Effective Java".into(),
        authors: vec!["Joshua Bloch".into()],
        year: Some("2018".into()),
        pages: Some(416),
        publisher: Some("Addison-Wesley".into()),
        description: Some("The definitive guide.".into()),
        cover_url: Some("https://books.google.com/x.jpg".into()),
        series: Some("The Java Series".into()),
        series_index: Some("3".into()),
        first_publish_year: Some(2001),
        genres: vec!["Computers".into(), "Java".into()],
        relevance: None,
    }
}

/// A source that answers for nothing beyond the two fields a candidate is
/// required to carry.
fn bare_edition() -> ProviderEdition {
    ProviderEdition {
        isbn10: None,
        authors: Vec::new(),
        year: None,
        pages: None,
        publisher: None,
        description: None,
        series: None,
        series_index: None,
        genres: Vec::new(),
        ..full_edition()
    }
}

// ── Completeness (AC6) ───────────────────────────────────────────

#[test]
fn every_field_in_all_reads_a_value_from_a_source_that_answers_for_everything() {
    // The compare view renders nothing but `ALL`, so this is the whole of
    // "adding a variant adds its row": a field that reaches `ALL` with a
    // working `source_value` has a working row, and one that doesn't fails
    // here rather than rendering a permanently-empty row in the UI.
    let edition = full_edition();
    for field in MetadataField::ALL {
        assert!(
            field.is_available(&edition),
            "{field:?} reads nothing from a source that answers for every field"
        );
        assert!(!field.label().is_empty(), "{field:?} has no row heading");
        assert!(!field.slug().is_empty(), "{field:?} has no testid slug");
    }
}

#[test]
fn every_field_in_all_stages_into_a_form_signal_that_actually_changes() {
    // Adding a variant to `ALL` is the one step the compiler can't demand, so
    // this drives each entry through the two accessors a mis-wired arm would
    // otherwise hide: `apply` writing to the wrong signal, or `current`
    // reading from one. Both render a row that looks right and copies the
    // wrong value.
    let edition = full_edition();
    for field in MetadataField::ALL {
        let (_dom, fields) = mounted_fields();
        let before = field.current(fields);
        field.apply(fields, &edition);
        let after = field.current(fields);

        assert_eq!(
            after,
            field.source_value(&edition),
            "{field:?}: `current` does not read back what `apply` wrote"
        );
        assert_ne!(
            before, after,
            "{field:?}: applying changed nothing, so its arm writes a signal `current` never reads"
        );
    }
}

#[test]
fn all_lists_each_field_exactly_once_and_slugs_are_distinct() {
    let mut slugs: Vec<String> = MetadataField::ALL.iter().map(|f| f.slug()).collect();
    let before = slugs.len();
    slugs.sort();
    slugs.dedup();
    assert_eq!(
        slugs.len(),
        before,
        "two fields share a testid slug, so a spec can't tell their rows apart"
    );

    let mut seen: Vec<MetadataField> = Vec::new();
    for field in MetadataField::ALL {
        assert!(!seen.contains(field), "{field:?} is listed twice");
        seen.push(*field);
    }
}

// ── The empty-source guard (AC4) ─────────────────────────────────

#[test]
fn a_field_the_source_lacks_is_unavailable_and_reads_empty() {
    let bare = bare_edition();
    // Title and ISBN-13 are required of every candidate; everything else on
    // this source is absent.
    for field in MetadataField::ALL {
        let expected_present = matches!(field, MetadataField::Title | MetadataField::Isbn13);
        assert_eq!(
            field.is_available(&bare),
            expected_present,
            "{field:?} availability on a source that answers for nothing"
        );
    }
}

#[test]
fn a_whitespace_only_provider_value_counts_as_absent() {
    // Otherwise "   " would be applicable, and applying it would blank a real
    // value while looking like a copy.
    let edition = ProviderEdition {
        publisher: Some("   ".into()),
        ..full_edition()
    };
    assert!(!MetadataField::Publisher.is_available(&edition));
    assert_eq!(MetadataField::Publisher.source_value(&edition), "");
}

#[test]
fn source_value_reads_the_editions_year_for_the_forms_published_field() {
    // The two names differ — `ProviderEdition::year` feeds the form's
    // `published` — which is exactly the kind of mapping a per-row hand-write
    // gets wrong.
    assert_eq!(
        MetadataField::Published.source_value(&full_edition()),
        "2018"
    );
}

#[test]
fn source_value_joins_the_list_fields_for_display() {
    assert_eq!(
        MetadataField::Genres.source_value(&full_edition()),
        "Computers, Java"
    );
    assert_eq!(
        MetadataField::Authors.source_value(&full_edition()),
        "Joshua Bloch"
    );
}

// ── Applying into the form signals ───────────────────────────────

/// Carries the harness's form signals out to the test so it can drive
/// `apply` and read the result from outside the component tree.
#[derive(Clone)]
struct Capture(Rc<RefCell<Option<FormFields>>>);

impl PartialEq for Capture {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

fn form_root(capture: Capture) -> Element {
    let fields = FormFields {
        title: use_signal(|| "Old Title".to_string()),
        description: use_signal(String::new),
        publisher: use_signal(|| "Old Publisher".to_string()),
        published: use_signal(String::new),
        language: use_signal(String::new),
        series: use_signal(String::new),
        series_index: use_signal(String::new),
        isbn13: use_signal(String::new),
        isbn10: use_signal(String::new),
        print_pages: use_signal(String::new),
        authors: use_signal(|| vec!["Old Author".to_string()]),
        tags: use_signal(|| vec!["keep-me".to_string()]),
        genres: use_signal(Vec::new),
        sort_by: use_signal(String::new),
        filename: use_signal(String::new),
    };
    use_hook(|| {
        *capture.0.borrow_mut() = Some(fields);
    });
    rsx! {
        div {}
    }
}

/// Mount the harness and hand back its form signals.
fn mounted_fields() -> (VirtualDom, FormFields) {
    let captured = Capture(Rc::new(RefCell::new(None)));
    let mut dom = VirtualDom::new_with_props(form_root, captured.clone());
    dom.rebuild_in_place();
    let fields = captured
        .0
        .borrow_mut()
        .take()
        .expect("form_root captures its signals on first mount");
    (dom, fields)
}

#[test]
fn apply_stages_one_field_and_leaves_every_other_alone() {
    let (_dom, fields) = mounted_fields();
    let edition = full_edition();

    MetadataField::Title.apply(fields, &edition);

    assert_eq!(*fields.title.read(), "Effective Java");
    // Untouched: applying one row must not move another.
    assert_eq!(*fields.publisher.read(), "Old Publisher");
    assert_eq!(*fields.authors.read(), vec!["Old Author".to_string()]);
}

#[test]
fn apply_writes_the_list_fields_as_lists_not_as_the_joined_display_string() {
    let (_dom, fields) = mounted_fields();
    let edition = full_edition();

    MetadataField::Authors.apply(fields, &edition);
    MetadataField::Genres.apply(fields, &edition);

    assert_eq!(*fields.authors.read(), vec!["Joshua Bloch".to_string()]);
    assert_eq!(
        *fields.genres.read(),
        vec!["Computers".to_string(), "Java".to_string()]
    );
}

#[test]
fn apply_is_a_no_op_for_a_field_the_source_has_no_value_for() {
    // The most damaging thing this screen could do is blank a value the
    // reader already had, so the guard is asserted on the *apply* path and
    // not only on the button that calls it.
    let (_dom, fields) = mounted_fields();
    let bare = bare_edition();

    MetadataField::Publisher.apply(fields, &bare);
    MetadataField::Authors.apply(fields, &bare);

    assert_eq!(*fields.publisher.read(), "Old Publisher");
    assert_eq!(*fields.authors.read(), vec!["Old Author".to_string()]);
}

#[test]
fn apply_stages_every_available_field_when_each_is_taken_in_turn() {
    // The "take everything from this source" path, field by field.
    let (_dom, fields) = mounted_fields();
    let edition = full_edition();

    for field in MetadataField::ALL {
        field.apply(fields, &edition);
    }

    assert_eq!(*fields.title.read(), "Effective Java");
    assert_eq!(*fields.publisher.read(), "Addison-Wesley");
    assert_eq!(*fields.published.read(), "2018");
    assert_eq!(*fields.series.read(), "The Java Series");
    assert_eq!(*fields.series_index.read(), "3");
    assert_eq!(*fields.isbn13.read(), "9780134685991");
    assert_eq!(*fields.isbn10.read(), "0134685997");
    assert_eq!(*fields.print_pages.read(), "416");
    assert_eq!(*fields.description.read(), "The definitive guide.");
    // Tags are not a copyable field: a provider's vocabulary must not
    // overwrite the EPUB's own `<dc:subject>` entries.
    assert_eq!(*fields.tags.read(), vec!["keep-me".to_string()]);
}

#[test]
fn current_reads_the_staged_form_value_so_a_row_reflects_an_apply_immediately() {
    let (_dom, fields) = mounted_fields();
    assert_eq!(MetadataField::Title.current(fields), "Old Title");

    MetadataField::Title.apply(fields, &full_edition());
    assert_eq!(MetadataField::Title.current(fields), "Effective Java");
}

#[test]
fn a_list_of_blank_entries_reads_as_absent_rather_than_as_the_separator() {
    // Joining first would make `["", ""]` render — and stage — as ", ",
    // which is a real value overwriting a real value.
    let edition = ProviderEdition {
        authors: vec!["  ".into(), String::new()],
        genres: vec![String::new()],
        ..full_edition()
    };
    assert_eq!(MetadataField::Authors.source_value(&edition), "");
    assert!(!MetadataField::Authors.is_available(&edition));
    assert!(!MetadataField::Genres.is_available(&edition));

    let (_dom, fields) = mounted_fields();
    MetadataField::Authors.apply(fields, &edition);
    assert_eq!(*fields.authors.read(), vec!["Old Author".to_string()]);
}

#[test]
fn apply_stages_the_trimmed_list_the_row_displayed_not_the_raw_one() {
    let edition = ProviderEdition {
        genres: vec!["  Computers  ".into(), "   ".into(), "Java".into()],
        ..full_edition()
    };
    assert_eq!(
        MetadataField::Genres.source_value(&edition),
        "Computers, Java"
    );

    let (_dom, fields) = mounted_fields();
    MetadataField::Genres.apply(fields, &edition);
    assert_eq!(
        *fields.genres.read(),
        vec!["Computers".to_string(), "Java".to_string()],
        "what is staged must match what the row showed"
    );
}
