//! Tests for the drill-in's delta/trend math and Finished-book mapping.

use omnibus_shared::PeriodComparison;

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

#[test]
fn build_histogram_bars_keeps_every_bucket_flat_when_nothing_was_rated() {
    let bars = build_histogram_bars(&(1..=10).map(|h| bucket(h, 0)).collect::<Vec<_>>());

    assert_eq!(bars.len(), 10);
    assert!(bars.iter().all(|b| b.height_pct == 0));
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
