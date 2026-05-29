use std::collections::BTreeMap;

use omnibus_shared::{EbookMetadata, ViewFilters};

#[derive(Clone, PartialEq)]
pub(crate) struct FacetCounts {
    pub(crate) authors: Vec<(String, usize)>,
    pub(crate) series: Vec<(String, usize)>,
    /// Normalized lowercase format keys (`"epub"`, `"m4b"`, …) paired with
    /// counts. The display label is derived at render time via
    /// [`format_display_label`] so the keys stay canonical.
    pub(crate) formats: Vec<(String, usize)>,
    pub(crate) tags: Vec<(String, usize)>,
}

fn matches_filters(book: &EbookMetadata, filters: &ViewFilters) -> bool {
    // Allocation-free membership checks: filter buckets are typically tiny
    // (a handful of selected chips), so a nested `any().any()` is faster
    // than building a fresh HashSet per book on every filter pass.
    if !filters.authors.is_empty()
        && !filters
            .authors
            .iter()
            .any(|a| book.creators.iter().any(|c| &c.name == a))
    {
        return false;
    }
    if !filters.series.is_empty() {
        let series = book.series.as_deref().unwrap_or("");
        if !filters.series.iter().any(|s| s == series) {
            return false;
        }
    }
    if !filters.formats.is_empty()
        && !filters
            .formats
            .iter()
            .any(|f| book.formats.iter().any(|bf| bf.eq_ignore_ascii_case(f)))
    {
        return false;
    }
    if !filters.tags.is_empty()
        && !filters
            .tags
            .iter()
            .any(|t| book.subjects.iter().any(|s| s == t))
    {
        return false;
    }
    true
}

pub(crate) fn apply_filters(books: &[EbookMetadata], filters: &ViewFilters) -> Vec<EbookMetadata> {
    if filters.is_empty() {
        return books.to_vec();
    }
    books
        .iter()
        .filter(|b| matches_filters(b, filters))
        .cloned()
        .collect()
}

pub(crate) fn facet_counts(books: &[EbookMetadata]) -> FacetCounts {
    let mut authors: BTreeMap<String, usize> = BTreeMap::new();
    let mut series: BTreeMap<String, usize> = BTreeMap::new();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for book in books {
        for c in &book.creators {
            *authors.entry(c.name.clone()).or_default() += 1;
        }
        if let Some(s) = book.series.as_deref() {
            if !s.is_empty() {
                *series.entry(s.to_string()).or_default() += 1;
            }
        }
        for fmt in &book.formats {
            let key = fmt.trim().to_ascii_lowercase();
            if !key.is_empty() {
                *formats.entry(key).or_default() += 1;
            }
        }
        for tag in &book.subjects {
            if !tag.is_empty() {
                *tags.entry(tag.clone()).or_default() += 1;
            }
        }
    }
    FacetCounts {
        authors: sorted_facet(authors),
        series: sorted_facet(series),
        formats: sorted_facet(formats),
        tags: sorted_facet(tags),
    }
}

/// User-facing label for a normalized format key. Recognized formats get a
/// friendly name (`"epub"` → `"ePub"`, `"m4b"` → `"Audiobook"`); anything
/// else passes through upper-cased.
pub(crate) fn format_display_label(key: &str) -> String {
    match key {
        "epub" => "ePub".to_string(),
        "m4b" => "Audiobook".to_string(),
        "pdf" => "PDF".to_string(),
        "mp3" => "Audiobook (MP3)".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// Short badge text for the table's Formats column. Stays compact so a row
/// with two formats doesn't overflow the cell.
pub(crate) fn format_badge_label(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

fn sorted_facet(map: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_shared::Contributor;

    #[allow(clippy::too_many_arguments)]
    fn book(
        id: i64,
        filename: &str,
        title: Option<&str>,
        authors: &[(&str, Option<&str>)],
        series: Option<(&str, &str)>,
        modified: Option<&str>,
        added_at: Option<&str>,
        subjects: &[&str],
    ) -> EbookMetadata {
        EbookMetadata {
            id,
            filename: filename.into(),
            title: title.map(Into::into),
            creators: authors
                .iter()
                .map(|(name, file_as)| Contributor {
                    name: (*name).into(),
                    role: None,
                    file_as: file_as.map(Into::into),
                    id: None,
                })
                .collect(),
            series: series.map(|(s, _)| s.into()),
            series_index: series.map(|(_, i)| i.into()),
            modified: modified.map(Into::into),
            added_at: added_at.map(Into::into),
            subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    fn ids(books: &[EbookMetadata]) -> Vec<i64> {
        books.iter().map(|b| b.id).collect()
    }

    fn sample() -> Vec<EbookMetadata> {
        vec![
            book(
                1,
                "alpha.epub",
                Some("Alpha"),
                &[("Tolkien, J.R.R.", Some("Tolkien, J.R.R."))],
                Some(("Foundation", "1")),
                Some("2024-01-01T00:00:00"),
                Some("2025-03-10T00:00:00"),
                &["Fantasy"],
            ),
            book(
                2,
                "beta.epub",
                Some("Beta"),
                &[("Asimov, Isaac", Some("Asimov, Isaac"))],
                Some(("Foundation", "2")),
                Some("2024-06-01T00:00:00"),
                Some("2025-01-05T00:00:00"),
                &["Sci-Fi"],
            ),
            book(
                3,
                "gamma.epub",
                Some("Gamma"),
                &[("Le Guin, Ursula", Some("Le Guin, Ursula"))],
                None,
                Some("2023-01-01T00:00:00"),
                Some("2025-02-20T00:00:00"),
                &["Fantasy", "Sci-Fi"],
            ),
        ]
    }

    // --- apply_filters ---

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

    // --- facet_counts ---

    #[test]
    fn facet_counts_orders_by_count_desc_then_name() {
        let s = sample();
        let f = facet_counts(&s);
        // Series: Foundation present once with count 2
        assert_eq!(f.series, vec![("Foundation".into(), 2)]);
        // Authors: each unique once
        assert_eq!(f.authors.len(), 3);
    }

    #[test]
    fn facet_counts_skips_empty_series_strings() {
        let mut b = sample();
        b[0].series = Some(String::new());
        let f = facet_counts(&b);
        assert!(f.series.iter().all(|(s, _)| !s.is_empty()));
    }

    // --- format filter ---

    fn with_formats(mut b: EbookMetadata, formats: &[&str]) -> EbookMetadata {
        b.formats = formats.iter().map(|s| (*s).to_string()).collect();
        b
    }

    #[test]
    fn format_counts_normalize_case_insensitively() {
        let books = vec![
            with_formats(sample()[0].clone(), &["EPUB"]),
            with_formats(sample()[1].clone(), &["epub", "m4b"]),
            with_formats(sample()[2].clone(), &["M4B"]),
        ];
        let f = facet_counts(&books);
        let formats: std::collections::HashMap<_, _> = f.formats.into_iter().collect();
        assert_eq!(formats.get("epub").copied(), Some(2));
        assert_eq!(formats.get("m4b").copied(), Some(2));
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
    fn format_display_label_friendly_names() {
        assert_eq!(format_display_label("epub"), "ePub");
        assert_eq!(format_display_label("m4b"), "Audiobook");
        assert_eq!(format_display_label("pdf"), "PDF");
        // Unknown formats fall through upper-cased.
        assert_eq!(format_display_label("azw3"), "AZW3");
    }

    // --- additional filter / facet / sort edge cases (#215) ---

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
    fn format_facet_counts_tally_across_full_unfiltered_list() {
        // facet_counts must accumulate over every book it is given (the full,
        // unfiltered list) — duplicates across books increment the same key.
        let books = vec![
            with_formats(sample()[0].clone(), &["epub"]),
            with_formats(sample()[1].clone(), &["epub", "m4b"]),
            with_formats(sample()[2].clone(), &["m4b"]),
        ];
        let f = facet_counts(&books);
        let formats: std::collections::HashMap<_, _> = f.formats.into_iter().collect();
        assert_eq!(formats.get("epub").copied(), Some(2));
        assert_eq!(formats.get("m4b").copied(), Some(2));
        assert_eq!(formats.len(), 2);
    }

    #[test]
    fn empty_filters_returns_full_list_unchanged_in_original_order() {
        // Acceptance (4): an empty ViewFilters yields the input list verbatim.
        let s = sample();
        let out = apply_filters(&s, &ViewFilters::default());
        assert_eq!(ids(&out), ids(&s));
    }
}
