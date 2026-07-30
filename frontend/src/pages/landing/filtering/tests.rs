//! Tests for the landing-page book filters: facet AND-across-groups /
//! OR-within-group semantics for tags, formats, and series, including
//! case-insensitive format matching and the empty-filter passthrough.

use super::*;
use omnibus_shared::Contributor;

struct BookSpec<'a> {
    id: i64,
    filename: &'a str,
    title: Option<&'a str>,
    authors: &'a [(&'a str, Option<&'a str>)],
    series: Option<(&'a str, &'a str)>,
    modified: Option<&'a str>,
    added_at: Option<&'a str>,
    subjects: &'a [&'a str],
}

fn book(s: BookSpec<'_>) -> EbookMetadata {
    EbookMetadata {
        id: s.id,
        filename: s.filename.into(),
        title: s.title.map(Into::into),
        creators: s
            .authors
            .iter()
            .map(|(name, file_as)| Contributor {
                name: (*name).into(),
                role: None,
                file_as: file_as.map(Into::into),
                id: None,
            })
            .collect(),
        series: s.series.map(|(name, _)| name.into()),
        series_index: s.series.map(|(_, idx)| idx.into()),
        modified: s.modified.map(Into::into),
        added_at: s.added_at.map(Into::into),
        subjects: s.subjects.iter().map(|sub| (*sub).to_string()).collect(),
        ..Default::default()
    }
}

fn ids(books: &[EbookMetadata]) -> Vec<i64> {
    books.iter().map(|b| b.id).collect()
}

fn sample() -> Vec<EbookMetadata> {
    vec![
        book(BookSpec {
            id: 1,
            filename: "alpha.epub",
            title: Some("Alpha"),
            authors: &[("Tolkien, J.R.R.", Some("Tolkien, J.R.R."))],
            series: Some(("Foundation", "1")),
            modified: Some("2024-01-01T00:00:00"),
            added_at: Some("2025-03-10T00:00:00"),
            subjects: &["Fantasy"],
        }),
        book(BookSpec {
            id: 2,
            filename: "beta.epub",
            title: Some("Beta"),
            authors: &[("Asimov, Isaac", Some("Asimov, Isaac"))],
            series: Some(("Foundation", "2")),
            modified: Some("2024-06-01T00:00:00"),
            added_at: Some("2025-01-05T00:00:00"),
            subjects: &["Sci-Fi"],
        }),
        book(BookSpec {
            id: 3,
            filename: "gamma.epub",
            title: Some("Gamma"),
            authors: &[("Le Guin, Ursula", Some("Le Guin, Ursula"))],
            series: None,
            modified: Some("2023-01-01T00:00:00"),
            added_at: Some("2025-02-20T00:00:00"),
            subjects: &["Fantasy", "Sci-Fi"],
        }),
    ]
}

#[test]
fn empty_filters_returns_all_books() {
    let s = sample();
    let out = apply_filters(&s, &ViewFilters::default());
    assert_eq!(ids(&out), vec![1, 2, 3]);
}

#[test]
fn single_facet_or_within_group() {
    let s = sample();
    let f = ViewFilters {
        authors: vec!["Tolkien, J.R.R.".into(), "Asimov, Isaac".into()],
        ..Default::default()
    };
    let out = apply_filters(&s, &f);
    assert_eq!(ids(&out), vec![1, 2]);
}

#[test]
fn multi_facet_and_across_groups() {
    let s = sample();
    let f = ViewFilters {
        authors: vec!["Tolkien, J.R.R.".into(), "Asimov, Isaac".into()],
        series: vec!["Foundation".into()],
        ..Default::default()
    };
    let out = apply_filters(&s, &f);
    // alpha is Tolkien + Foundation, beta is Asimov + Foundation, gamma
    // is Le Guin with no series — so only alpha + beta survive the AND
    // across (authors, series).
    assert_eq!(ids(&out), vec![1, 2]);
}

#[test]
fn series_filter_excludes_books_with_no_series() {
    let s = sample();
    let f = ViewFilters {
        series: vec!["Foundation".into()],
        ..Default::default()
    };
    let out = apply_filters(&s, &f);
    assert_eq!(ids(&out), vec![1, 2]);
}

fn with_formats(mut b: EbookMetadata, formats: &[&str]) -> EbookMetadata {
    b.formats = formats.iter().map(|s| (*s).to_string()).collect();
    b
}

#[test]
fn empty_formats_filter_keeps_all_books() {
    let books = vec![
        with_formats(sample()[0].clone(), &["EPUB"]),
        with_formats(sample()[1].clone(), &["m4b"]),
    ];
    let out = apply_filters(&books, &ViewFilters::default());
    assert_eq!(ids(&out), vec![1, 2]);
}

#[test]
fn format_filter_or_within_bucket() {
    let books = vec![
        with_formats(sample()[0].clone(), &["epub"]),
        with_formats(sample()[1].clone(), &["m4b"]),
        with_formats(sample()[2].clone(), &["pdf"]),
    ];
    let f = ViewFilters {
        formats: vec!["epub".into(), "m4b".into()],
        ..Default::default()
    };
    let out = apply_filters(&books, &f);
    assert_eq!(ids(&out), vec![1, 2]);
}

#[test]
fn format_filter_matches_case_insensitively() {
    // A book whose persisted format string is upper-case "EPUB" should
    // still match a filter chip whose normalized key is "epub".
    let books = vec![with_formats(sample()[0].clone(), &["EPUB"])];
    let f = ViewFilters {
        formats: vec!["epub".into()],
        ..Default::default()
    };
    let out = apply_filters(&books, &f);
    assert_eq!(ids(&out), vec![1]);
}

#[test]
fn format_filter_intersects_with_other_facets() {
    // Tolkien wrote `alpha` (Fantasy + Foundation) and only that book is
    // EPUB — a Tolkien + EPUB filter should leave just alpha.
    let books = vec![
        with_formats(sample()[0].clone(), &["epub"]),
        with_formats(sample()[1].clone(), &["m4b"]),
    ];
    let f = ViewFilters {
        authors: vec!["Tolkien, J.R.R.".into()],
        formats: vec!["epub".into()],
        ..Default::default()
    };
    let out = apply_filters(&books, &f);
    assert_eq!(ids(&out), vec![1]);
}

#[test]
fn tag_filter_is_or_within_bucket_not_and() {
    // tags is OR within the bucket: selecting two subjects must keep books
    // carrying *either* one, never require *both* (AND would wrongly drop
    // the single-tag books). alpha=Fantasy, beta=Sci-Fi, gamma=both.
    let s = sample();
    let both = ViewFilters {
        tags: vec!["Fantasy".into(), "Sci-Fi".into()],
        ..Default::default()
    };
    // OR keeps all three; AND would have returned only gamma (id 3).
    assert_eq!(ids(&apply_filters(&s, &both)), vec![1, 2, 3]);

    // A single selected tag still matches every book carrying it.
    let one = ViewFilters {
        tags: vec!["Fantasy".into()],
        ..Default::default()
    };
    assert_eq!(ids(&apply_filters(&s, &one)), vec![1, 3]);
}

#[test]
fn multi_format_filter_is_or_within_bucket_not_and() {
    // Guards against the bug called out in #215: selecting two formats must
    // include books matching *either* (OR within the formats bucket), never
    // require a book to carry *both* (AND would wrongly exclude single-format
    // books). Each book here has exactly one format.
    let books = vec![
        with_formats(sample()[0].clone(), &["epub"]),
        with_formats(sample()[1].clone(), &["m4b"]),
        with_formats(sample()[2].clone(), &["pdf"]),
    ];
    let f = ViewFilters {
        formats: vec!["epub".into(), "m4b".into()],
        ..Default::default()
    };
    let out = apply_filters(&books, &f);
    // OR semantics keep both single-format epub and m4b books; AND would
    // have dropped both and returned an empty list.
    assert_eq!(ids(&out), vec![1, 2]);
}

#[test]
fn empty_filters_returns_full_list_unchanged_in_original_order() {
    // An empty ViewFilters yields the input list verbatim.
    let s = sample();
    assert_eq!(
        ids(&apply_filters(&s, &ViewFilters::default())),
        vec![1, 2, 3]
    );
}

#[test]
fn format_badge_label_uppercases() {
    assert_eq!(format_badge_label(" epub "), "EPUB");
    assert_eq!(format_badge_label("m4b"), "M4B");
}
