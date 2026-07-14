use super::*;

fn author(name: &str, sort: Option<&str>) -> AuthorSummary {
    AuthorSummary {
        id: 0,
        name: name.into(),
        sort: sort.map(Into::into),
        book_count: 0,
        accent: None,
        has_photo: false,
    }
}

#[test]
fn sort_key_uses_comma_form_sort_verbatim() {
    let a = author("Louisa May Alcott", Some("Alcott, Louisa May"));
    assert_eq!(sort_key(&a), "Alcott, Louisa May");
    assert_eq!(first_letter(&a), 'A');
}

#[test]
fn sort_key_ignores_commaless_sort_and_falls_back_to_name() {
    // Regression: a comma-less `sort` value (some real-world dumps
    // stuff a pseudonym or bare surname here, e.g. "Underwood" for
    // an author whose display name is "Erin A. Craig") used to win,
    // grouping Erin under U. Now we treat it as garbage and derive
    // the surname from the display name instead.
    let a = author("Erin A. Craig", Some("Underwood"));
    assert_eq!(sort_key(&a), "Craig, Erin A.");
    assert_eq!(first_letter(&a), 'C');
}

#[test]
fn sort_key_falls_back_to_name_when_sort_missing() {
    let a = author("Ada Lovelace", None);
    assert_eq!(sort_key(&a), "Lovelace, Ada");
    assert_eq!(first_letter(&a), 'L');
}

#[test]
fn sort_key_treats_lastname_first_display_name_verbatim() {
    // Some Calibre dumps store the display name itself as
    // "Surname, Given". Keep it as-is — `rsplitn(2, ' ')` would
    // otherwise treat "Olivia" as the surname.
    let a = author("Atwater, Olivia", None);
    assert_eq!(sort_key(&a), "Atwater, Olivia");
    assert_eq!(first_letter(&a), 'A');
}

#[test]
fn sort_key_handles_mononym() {
    let a = author("Plato", None);
    assert_eq!(sort_key(&a), "Plato");
    assert_eq!(first_letter(&a), 'P');
}

#[test]
fn sort_key_handles_empty_sort_string() {
    // Explicit empty string in `sort` should be ignored the same
    // way as `None`.
    let a = author("Ada Lovelace", Some(""));
    assert_eq!(sort_key(&a), "Lovelace, Ada");
    assert_eq!(first_letter(&a), 'L');
}

#[test]
fn first_letter_buckets_non_alpha_under_hash() {
    // Digits, punctuation, and non-Latin scripts collapse into the
    // single '#' section after Z. The test names use mononyms so
    // `sort_key` doesn't pull a Latin surname out of a multi-word
    // string (e.g. "1984 Editorial" would derive "Editorial, 1984"
    // and land under E — which is correct, but not what this test
    // is checking).
    //
    // We also avoid Latin diacritics here because their byte form
    // depends on source-file Unicode normalization — `chars().next()`
    // on NFD-encoded "Émile" returns plain `E`, which would land
    // under E. Either bucket is defensible; what matters is that
    // unambiguously-non-Latin starts land under '#'.
    assert_eq!(first_letter(&author("1984", None)), '#');
    assert_eq!(first_letter(&author("[Anonymous]", None)), '#');
    assert_eq!(first_letter(&author("\u{4E2D}\u{6587}", None)), '#');
}

#[test]
fn is_non_alpha_key_classifies_keys() {
    assert!(!is_non_alpha_key("Alcott, Louisa May"));
    assert!(!is_non_alpha_key("zZz"));
    assert!(is_non_alpha_key("1984"));
    assert!(is_non_alpha_key("[Anonymous]"));
    assert!(is_non_alpha_key("\u{4E2D}\u{6587}"));
    assert!(is_non_alpha_key(""));
}
