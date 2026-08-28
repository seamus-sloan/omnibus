//! Tests for the drill-in's delta/trend math and Finished-book mapping.

use omnibus_shared::{PeriodComparison, TrendPoint};

use super::*;

#[test]
fn percent_delta_reports_new_when_previous_was_zero_and_current_is_positive() {
    let d = percent_delta(3.0, 0.0).unwrap();
    assert_eq!(d.label, "New");
    assert_eq!(d.css_class, "up");
}

#[test]
fn percent_delta_is_none_when_both_windows_are_zero() {
    assert!(percent_delta(0.0, 0.0).is_none());
}

#[test]
fn percent_delta_rounds_and_signs_the_change() {
    let up = percent_delta(124.0, 100.0).unwrap();
    assert_eq!(up.label, "24%");
    assert_eq!(up.css_class, "up");

    let down = percent_delta(80.0, 100.0).unwrap();
    assert_eq!(down.label, "20%");
    assert_eq!(down.css_class, "down");
}

#[test]
fn percent_delta_flags_no_change_within_half_a_percent() {
    let d = percent_delta(100.2, 100.0).unwrap();
    assert_eq!(d.label, "No change");
    assert_eq!(d.css_class, "flat");
}

#[test]
fn stars_delta_is_none_without_a_rating_on_either_side() {
    assert!(stars_delta(Some(4.0), None).is_none());
    assert!(stars_delta(None, Some(4.0)).is_none());
    assert!(stars_delta(None, None).is_none());
}

#[test]
fn stars_delta_formats_the_absolute_star_change() {
    let up = stars_delta(Some(4.5), Some(4.0)).unwrap();
    assert_eq!(up.label, "0.5\u{2605}");
    assert_eq!(up.css_class, "up");

    let down = stars_delta(Some(3.5), Some(4.0)).unwrap();
    assert_eq!(down.label, "0.5\u{2605}");
    assert_eq!(down.css_class, "down");
}

#[test]
fn build_trend_bars_scales_to_the_tallest_point_and_stays_zero_when_empty_of_activity() {
    let points = vec![
        ("A".to_string(), 0.0),
        ("B".to_string(), 2.0),
        ("C".to_string(), 4.0),
    ];
    let bars = build_trend_bars(&points);
    assert_eq!(bars[0].height_pct, 0);
    assert_eq!(bars[1].height_pct, 50);
    assert_eq!(bars[2].height_pct, 100);

    let zeroed = build_trend_bars(&[("A".to_string(), 0.0)]);
    assert_eq!(zeroed[0].height_pct, 0);
}

fn bucket(half_stars: i64, books: i64) -> RatingBucket {
    RatingBucket { half_stars, books }
}

#[test]
fn star_label_renders_buckets_in_stars_never_in_half_stars() {
    assert_eq!(star_label(&bucket(1, 0)), "0.5");
    assert_eq!(star_label(&bucket(2, 0)), "1");
    assert_eq!(star_label(&bucket(7, 0)), "3.5");
    assert_eq!(star_label(&bucket(10, 0)), "5");
}

#[test]
fn build_histogram_bars_normalizes_counts_and_titles_each_bar_with_its_total() {
    let bars = build_histogram_bars(&[bucket(1, 1), bucket(2, 0), bucket(10, 4)]);

    assert_eq!(bars[0].height_pct, 25);
    assert_eq!(bars[1].height_pct, 0, "an empty bucket keeps its column");
    assert_eq!(bars[2].height_pct, 100);
    assert_eq!(bars[0].title, "0.5 \u{2605} \u{00B7} 1 book");
    assert_eq!(bars[1].title, "1 \u{2605} \u{00B7} 0 books");
    assert_eq!(bars[2].title, "5 \u{2605} \u{00B7} 4 books");
}

/// Whether a rendered chunk carries an exact testid — `stats-drill-histogram`
/// is a prefix of `stats-drill-histogram-empty`, so a bare `contains` on the
/// shorter name matches the empty state too.
#[cfg(feature = "server")]
fn has_testid(html: &str, testid: &str) -> bool {
    html.contains(&format!(r#""{testid}""#))
}

#[cfg(feature = "server")]
#[test]
fn render_histogram_shows_the_empty_state_rather_than_ten_flat_bars() {
    // The window carries no ratings. Ten zero-height columns would draw a
    // chart of nothing and read as a real distribution that happens to be
    // flat, so the drill-in says so in words instead.
    let none_rated = (1..=10).map(|h| bucket(h, 0)).collect::<Vec<_>>();
    let html = crate::test_support::render(render_histogram(&none_rated));
    assert!(has_testid(&html, "stats-drill-histogram-empty"), "{html}");
    assert!(!has_testid(&html, "stats-drill-histogram"), "{html}");

    // One rating anywhere is enough to be worth drawing.
    let mut rated = none_rated;
    rated[6] = bucket(7, 1);
    let html = crate::test_support::render(render_histogram(&rated));
    assert!(has_testid(&html, "stats-drill-histogram"), "{html}");
    assert!(!has_testid(&html, "stats-drill-histogram-empty"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn render_histogram_reuses_the_trend_chart_renderer() {
    // The histogram is the trend strip with a different x-axis. A private copy
    // of the bar markup would drift from it silently, so this pins that both
    // come out of `render_bars`.
    let bars = build_trend_bars(&[("J".to_string(), 1.0)]);
    let trend = crate::test_support::render(render_trend(Metric::AvgRating, &bars));
    let histogram = crate::test_support::render(render_histogram(&[bucket(10, 1)]));

    for class in ["st-drill-trend", "st-drill-trend-col", "st-drill-trend-bar"] {
        assert!(trend.contains(class), "trend missing {class}: {trend}");
        assert!(
            histogram.contains(class),
            "histogram missing {class}: {histogram}"
        );
    }
}

#[test]
fn delta_for_is_none_for_lifetime_regardless_of_metric() {
    let summary = StatsSummary::default();
    assert!(delta_for(Metric::Finished, &summary, StatsRange::AllTime).is_none());
    assert!(delta_for(Metric::Listening, &summary, StatsRange::AllTime).is_none());
}

#[test]
fn delta_for_finished_compares_against_the_previous_window() {
    let summary = StatsSummary {
        books_finished: 3,
        previous: PeriodComparison {
            books_finished: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let d = delta_for(Metric::Finished, &summary, StatsRange::Month).unwrap();
    assert_eq!(d.label, "50%");
    assert_eq!(d.css_class, "up");
}

#[test]
fn vs_label_is_empty_only_for_all_time() {
    assert_eq!(vs_label(StatsRange::Week), "vs last week");
    assert_eq!(vs_label(StatsRange::AllTime), "");
}

#[test]
fn short_month_and_short_day_fall_back_on_malformed_input() {
    assert_eq!(short_month("2026-07"), "J");
    assert_eq!(short_month("garbage"), "?");
    assert_eq!(short_day("2026-07-14"), "14");
    assert_eq!(short_day("garbage"), "?");
}

#[test]
fn rate_value_keeps_a_decimal_only_below_ten_pages_an_hour() {
    // Rounds half away from zero, like `avg_stars_value` — `{:.1}` alone would
    // give 4.2 here on round-half-to-even.
    assert_eq!(rate_value(4.25), "4.3");
    assert_eq!(rate_value(9.94), "9.9");
    // The branch is on the rounded figure: one decimal would read "10.0",
    // which isn't "under ten" however it got there.
    assert_eq!(rate_value(9.96), "10");
    assert_eq!(rate_value(32.4), "32");
    assert_eq!(rate_value(32.6), "33");
}

#[test]
fn finished_book_as_ebook_carries_title_author_and_cover() {
    let book = FinishedBook {
        book_uuid: "u1".to_string(),
        title: "Dune".to_string(),
        author: Some("Frank Herbert".to_string()),
        finished_at: 0,
        cover_url: Some("/api/covers/u1".to_string()),
        rating: Some(4.5),
    };
    let ebook = finished_book_as_ebook(&book);
    assert_eq!(ebook.title.as_deref(), Some("Dune"));
    assert_eq!(ebook.creators[0].name, "Frank Herbert");
    assert_eq!(ebook.unique_identifier.as_deref(), Some("u1"));
    assert_eq!(ebook.cover_url.as_deref(), Some("/api/covers/u1"));
}

#[test]
fn trend_points_for_pages_reads_the_per_day_ledger_series() {
    let mut summary = StatsSummary {
        pages_detail: PagesReadDetail {
            daily: vec![
                TrendPoint {
                    label: "2026-08-03".to_string(),
                    value: 41.0,
                },
                TrendPoint {
                    label: "2026-08-04".to_string(),
                    value: 12.0,
                },
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    summary.pages_read = Some(53);

    let points = trend_points(Metric::Pages, &summary);

    assert_eq!(
        points,
        vec![("03".to_string(), 41.0), ("04".to_string(), 12.0)]
    );
}

#[test]
fn delta_for_pages_compares_against_the_previous_windows_pages() {
    let summary = StatsSummary {
        pages_read: Some(120),
        previous: PeriodComparison {
            pages_read: 100,
            ..Default::default()
        },
        ..Default::default()
    };

    let d = delta_for(Metric::Pages, &summary, StatsRange::Month).unwrap();

    assert_eq!(d.label, "20%");
    assert_eq!(d.css_class, "up");
}

#[test]
fn delta_for_pages_treats_an_unmeasured_window_as_zero_not_as_missing() {
    // `None` is "nothing measurable happened", which against a real baseline is
    // a drop to zero — not an absent comparison.
    let summary = StatsSummary {
        pages_read: None,
        previous: PeriodComparison {
            pages_read: 200,
            ..Default::default()
        },
        ..Default::default()
    };

    let d = delta_for(Metric::Pages, &summary, StatsRange::Month).unwrap();

    assert_eq!(d.label, "100%");
    assert_eq!(d.css_class, "down");
}

#[test]
fn measured_line_singularizes_a_one_book_window() {
    let one = PagesReadDetail {
        measured_books: 1,
        ..Default::default()
    };
    assert_eq!(measured_line(&one), "Across 1 book this period.");
    let many = PagesReadDetail {
        measured_books: 4,
        ..Default::default()
    };
    assert_eq!(measured_line(&many), "Across 4 books this period.");
}

#[test]
fn unmeasured_line_names_the_books_the_total_leaves_out() {
    assert!(unmeasured_line(1, true).starts_with("1 more book has"));
    assert!(unmeasured_line(3, true).starts_with("3 more books have"));
    // Nothing was measured, so the line above it reported no page progress at
    // all — "more" would have no antecedent, and the singular needs "it".
    assert_eq!(
        unmeasured_line(1, false),
        "1 book has no known length yet, so nothing it contributed is counted."
    );
    assert!(unmeasured_line(3, false).starts_with("3 books have"));
    assert!(unmeasured_line(3, false).ends_with("nothing they contributed is counted."));
}

#[test]
fn cutover_line_warns_only_when_the_window_reaches_past_the_epoch() {
    // A Month window starting after the epoch is fully covered, so the date is
    // context; a Lifetime one is partly unmeasurable and has to say so.
    assert_eq!(
        cutover_line("2026-08-01", false),
        "Page tracking began 2026-08-01."
    );
    assert!(cutover_line("2026-08-01", true).contains("only partly covered"));
}

#[test]
fn predates_ledger_follows_the_servers_overlap_answer_not_the_range() {
    assert!(PagesReadDetail {
        since_day: Some("2026-08-01".to_string()),
        window_predates_ledger: true,
        ..Default::default()
    }
    .predates_ledger());
    // A window that opens after the epoch gets the date as context and no
    // caveat, whichever range produced it.
    assert!(!PagesReadDetail {
        since_day: Some("2026-08-01".to_string()),
        ..Default::default()
    }
    .predates_ledger());
    // No epoch recorded, nothing to warn about.
    assert!(!PagesReadDetail::default().predates_ledger());
}
