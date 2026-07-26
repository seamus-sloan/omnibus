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

#[test]
fn filter_authors_returns_all_when_query_is_empty() {
    let all = [
        author("Ada Lovelace", None),
        author("Louisa May Alcott", None),
    ];
    let out = filter_authors(&all, "");
    assert_eq!(out.len(), 2);
}

#[test]
fn filter_authors_matches_lowercase_query_against_mixed_case_name() {
    // `filter_authors` lowercases each author's name before comparing, but
    // takes `query` as-is — callers (e.g. `AuthorsIndexPage`) are
    // responsible for lowercasing the query first.
    let all = [
        author("Ada Lovelace", None),
        author("Louisa May Alcott", None),
    ];
    let out = filter_authors(&all, "lovelace");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "Ada Lovelace");
}

#[test]
fn sort_authors_by_name_orders_alpha_before_non_alpha_bucket() {
    let all = [author("1984", None), author("Ada Lovelace", None)];
    let mut filtered: Vec<&AuthorSummary> = all.iter().collect();
    sort_authors(&mut filtered, IndexSort::Name);
    assert_eq!(filtered[0].name, "Ada Lovelace");
    assert_eq!(filtered[1].name, "1984");
}

#[test]
fn sort_authors_by_book_count_orders_descending_then_name() {
    let mut low = author("Alpha", None);
    low.book_count = 3;
    let mut high = author("Bravo", None);
    high.book_count = 7;
    let all = [low, high];
    let mut filtered: Vec<&AuthorSummary> = all.iter().collect();
    sort_authors(&mut filtered, IndexSort::BookCount);
    assert_eq!(filtered[0].name, "Bravo");
    assert_eq!(filtered[1].name, "Alpha");
}

#[test]
fn group_by_letter_buckets_by_first_letter_when_shown() {
    // `group_by_letter` only merges *consecutive* same-letter entries — it
    // relies on the caller (the `use_memo` pipeline in `AuthorsIndexPage`)
    // to have already run `sort_authors` first. Both surnames here
    // ("Lovelace", "Lowe") start with 'L' and are given already in sorted
    // order, matching that contract.
    let all = [author("Ada Lovelace", None), author("Bob Lowe", None)];
    let groups = group_by_letter(&all, true);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].0, 'L');
    assert_eq!(groups[0].1.len(), 2);
}

#[test]
fn group_by_letter_returns_empty_when_show_letters_is_false() {
    let all = [author("Ada Lovelace", None)];
    let groups = group_by_letter(&all, false);
    assert!(groups.is_empty());
}
