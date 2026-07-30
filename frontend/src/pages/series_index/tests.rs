//! Tests for the series-index sort/filter helpers: `sort_key`'s leading-
//! "The" stripping and explicit-sort-field preference, and
//! `apply_filter_and_sort`'s name/author search and book-count ordering.

use super::*;

fn summary(name: &str, sort: Option<&str>) -> SeriesSummary {
    SeriesSummary {
        name: name.to_string(),
        sort: sort.map(String::from),
        ..Default::default()
    }
}

#[test]
fn sort_key_prefers_explicit_sort_field() {
    let s = summary("The Expanse", Some("Expanse"));
    assert_eq!(sort_key(&s), "Expanse");
}

#[test]
fn sort_key_ignores_empty_sort_field() {
    let s = summary("Foundation", Some(""));
    assert_eq!(sort_key(&s), "Foundation");
}

#[test]
fn sort_key_strips_leading_the_in_both_cases() {
    assert_eq!(sort_key(&summary("The Foo Bar", None)), "Foo Bar");
    assert_eq!(sort_key(&summary("the foo bar", None)), "foo bar");
}

#[test]
fn sort_key_leaves_non_the_prefix_intact() {
    // "Theology" must not have "The" stripped — only the "The " word.
    assert_eq!(sort_key(&summary("Theology", None)), "Theology");
    assert_eq!(sort_key(&summary("Dune", None)), "Dune");
}

fn summary_full(name: &str, author: Option<&str>, book_count: usize) -> SeriesSummary {
    SeriesSummary {
        name: name.to_string(),
        primary_author: author.map(String::from),
        book_count,
        ..Default::default()
    }
}

#[test]
fn apply_filter_and_sort_returns_all_when_query_is_empty() {
    let items = vec![
        summary_full("Foundation", Some("Asimov"), 7),
        summary_full("Dune", Some("Herbert"), 6),
    ];
    let out = apply_filter_and_sort(&items, "", IndexSort::Name);
    assert_eq!(out.len(), 2);
    // IndexSort::Name orders alphabetically by sort_key.
    assert_eq!(out[0].name, "Dune");
    assert_eq!(out[1].name, "Foundation");
}

#[test]
fn apply_filter_and_sort_filters_by_name_case_insensitive() {
    let items = vec![
        summary_full("Foundation", Some("Asimov"), 7),
        summary_full("Dune", Some("Herbert"), 6),
    ];
    let out = apply_filter_and_sort(&items, "FOUND", IndexSort::Name);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "Foundation");
}

#[test]
fn apply_filter_and_sort_filters_by_primary_author() {
    let items = vec![
        summary_full("Foundation", Some("Asimov"), 7),
        summary_full("Dune", Some("Herbert"), 6),
    ];
    let out = apply_filter_and_sort(&items, "herbert", IndexSort::Name);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "Dune");
}

#[test]
fn apply_filter_and_sort_sorts_by_book_count_descending_then_name() {
    let items = vec![
        summary_full("Alpha", None, 3),
        summary_full("Bravo", None, 7),
        summary_full("Charlie", None, 7),
    ];
    let out = apply_filter_and_sort(&items, "", IndexSort::BookCount);
    assert_eq!(out[0].name, "Bravo");
    assert_eq!(out[1].name, "Charlie");
    assert_eq!(out[2].name, "Alpha");
}
