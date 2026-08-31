use super::*;

fn share(name: &str, books: i64) -> GenreShare {
    GenreShare {
        name: name.into(),
        books,
    }
}

#[test]
fn fold_shares_keeps_top_four_and_folds_the_tail_into_other() {
    let shares: Vec<GenreShare> = (1..=6).map(|i| share(&format!("g{i}"), 7 - i)).collect();
    let folded = fold_shares(&shares);
    assert_eq!(folded.len(), 5);
    assert_eq!(folded[0], ("g1".to_string(), 6));
    assert_eq!(folded[4], ("Other".to_string(), 3));

    assert!(fold_shares(&[]).is_empty());
    assert_eq!(fold_shares(&[share("solo", 2)]).len(), 1);
}

#[test]
fn percentages_always_sum_to_one_hundred() {
    for counts in [
        vec![1, 1, 1],
        vec![2, 1],
        vec![7, 2, 1],
        vec![1, 1, 1, 1, 1, 1, 1],
    ] {
        let p = percentages(&counts);
        assert_eq!(p.iter().sum::<i64>(), 100, "counts {counts:?} → {p:?}");
    }
    assert_eq!(percentages(&[3, 1]), vec![75, 25]);
    assert_eq!(percentages(&[]), Vec::<i64>::new());
    assert_eq!(percentages(&[0, 0]), vec![0, 0]);
}

#[test]
fn untagged_note_singularizes_one_book() {
    assert_eq!(untagged_note(1), "+1 book without a genre");
    assert_eq!(untagged_note(4), "+4 books without a genre");
}

#[test]
fn fold_shares_other_covers_the_whole_tail_the_server_sent() {
    // The server no longer truncates to a top-8, so "Other" is the real
    // remainder rather than ranks five through eight.
    let shares: Vec<GenreShare> = (1..=12).map(|i| share(&format!("g{i}"), 1)).collect();
    let folded = fold_shares(&shares);
    assert_eq!(folded.len(), 5);
    assert_eq!(folded[4], ("Other".to_string(), 8));
    let pct = percentages(&folded.iter().map(|(_, c)| *c).collect::<Vec<_>>());
    assert_eq!(pct.iter().sum::<i64>(), 100);
}

#[test]
fn donut_gradient_builds_cumulative_stops() {
    let g = donut_gradient(&[36, 28, 20, 16]);
    assert_eq!(
        g,
        "conic-gradient(var(--st-donut-c0) 0% 36%, var(--st-donut-c1) 36% 64%, \
         var(--st-donut-c2) 64% 84%, var(--st-donut-c3) 84% 100%)"
    );
}

#[test]
fn donut_gradient_collapses_an_empty_genre_instead_of_leaving_a_hairline() {
    // Cumulative stops mean a zero-percent slice starts and ends at the same
    // angle. Per-slice widths would leave its colour showing for a fraction
    // of a degree where the next one begins.
    let g = donut_gradient(&[60, 0, 40]);
    assert!(g.contains("var(--st-donut-c1) 60% 60%"), "{g}");
    assert!(g.contains("var(--st-donut-c2) 60% 100%"), "{g}");
}

#[cfg(feature = "server")]
fn length_summary(buckets: &[(&str, i64)]) -> StatsSummary {
    StatsSummary {
        length_buckets: buckets
            .iter()
            .map(|(label, books)| omnibus_shared::LengthBucket {
                label: (*label).to_string(),
                books: *books,
            })
            .collect(),
        ..Default::default()
    }
}

#[cfg(feature = "server")]
#[test]
fn length_rows_render_every_bucket_the_server_sent_with_its_count() {
    let summary = length_summary(&[
        ("Under 300", 3),
        ("300\u{2013}499", 2),
        ("500+", 0),
        ("Unknown", 1),
    ]);
    let html = crate::test_support::render(rsx! { LengthRows { summary } });

    // Including the empty bucket — a missing bar reads as a different
    // distribution — and the unknown one, which is the whole point: an
    // audiobook has no page analogue and must not be filed as short.
    for label in ["Under 300", "300\u{2013}499", "500+", "Unknown"] {
        assert!(html.contains(label), "missing {label}: {html}");
    }
    assert!(html.contains("stats-length-split"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn length_rows_say_nothing_was_finished_rather_than_drawing_flat_bars() {
    // The server sends the zero-filled spine whether or not anything was
    // finished, so the rows decide on the total. Four zero-width bars would
    // read as a real distribution over no books.
    let summary = length_summary(&[("Under 300", 0), ("500+", 0), ("Unknown", 0)]);
    let html = crate::test_support::render(rsx! { LengthRows { summary } });

    assert!(
        html.contains("No books finished in this period yet."),
        "{html}"
    );
    assert!(!html.contains("st-split-track"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn genre_donut_carries_the_read_listen_split_along_its_foot() {
    // One card, not two: the ring says what was read and the bars say how, and
    // splitting them left a reader comparing two cards to answer one question.
    let summary = StatsSummary {
        genre_share: vec![share("History", 3), share("Essays", 1)],
        genre_tagged_books: 4,
        books_active: 5,
        reading_seconds: 6_400,
        listening_seconds: 3_600,
        ..Default::default()
    };
    let html = crate::test_support::render(rsx! { GenreDonut { summary } });

    assert!(html.contains("Where the time went"), "{html}");
    assert!(html.contains("stats-format-split"), "{html}");
    assert!(html.contains("Listened"), "{html}");
    // The book the ring cannot describe is disclosed, not absorbed.
    assert!(html.contains("+1 book without a genre"), "{html}");
}
