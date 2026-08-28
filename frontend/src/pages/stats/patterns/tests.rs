//! Tests for the time-pattern strips' normalization, labelling, and the
//! unzoned-activity disclosure.

use super::*;

fn hours(seconds: [i64; 24]) -> Vec<HourBucket> {
    seconds
        .into_iter()
        .enumerate()
        .map(|(hour, seconds)| HourBucket {
            hour: hour as i64,
            seconds,
        })
        .collect()
}

#[test]
fn normalize_scales_against_the_series_maximum() {
    assert_eq!(normalize(&[0, 50, 100]), vec![0, 50, 100]);
    assert_eq!(normalize(&[10, 20, 40]), vec![25, 50, 100]);
}

#[test]
fn normalize_keeps_an_all_zero_series_flat_rather_than_full_height() {
    // The strips are fixed-width, so an empty period must not normalize into
    // 24 full-height bars claiming a perfectly even day.
    assert_eq!(normalize(&[0, 0, 0]), vec![0, 0, 0]);
}

#[test]
fn duration_label_drops_the_unit_that_would_read_as_zero() {
    assert_eq!(duration_label(15_120), "4h 12m");
    assert_eq!(duration_label(2_100), "35m");
    assert_eq!(duration_label(50), "50s");
    // Whole hours and the empty column's title are where this used to drift
    // from iOS `Format.humanDuration`, which renders them "1h" and "0m".
    assert_eq!(duration_label(3_600), "1h");
    assert_eq!(duration_label(0), "0m");
}

#[test]
fn hour_title_reads_as_a_clock_time_beside_its_magnitude() {
    assert_eq!(hour_title(21, 15_120), "21:00 \u{00B7} 4h 12m");
    assert_eq!(hour_title(4, 60), "04:00 \u{00B7} 1m");
}

#[test]
fn hour_columns_label_every_third_hour_and_leave_the_rest_blank() {
    let mut seconds = [0_i64; 24];
    seconds[21] = 3_600;
    let cols = hour_columns(&hours(seconds));

    assert_eq!(cols.len(), 24, "all 24 columns render, zeros included");
    assert_eq!(cols[0].label, "00");
    assert_eq!(cols[1].label, "");
    assert_eq!(cols[3].label, "03");
    assert_eq!(cols[21].height_pct, 100);
    assert_eq!(cols[20].height_pct, 0);
}

#[test]
fn weekday_columns_use_the_servers_labels_rather_than_an_index_lookup() {
    let buckets = vec![
        WeekdayBucket {
            weekday: 0,
            label: "Mon".into(),
            seconds: 600,
        },
        WeekdayBucket {
            weekday: 6,
            label: "Sun".into(),
            seconds: 1_200,
        },
    ];
    let cols = weekday_columns(&buckets);

    assert_eq!(cols[0].label, "Mon");
    assert_eq!(cols[1].label, "Sun");
    assert_eq!(cols[0].height_pct, 50);
    assert_eq!(cols[1].title, "Sun \u{00B7} 20m");
}

#[test]
fn unzoned_note_is_absent_when_every_second_could_be_placed() {
    assert!(unzoned_note(0).is_none());
}

#[test]
fn unzoned_note_states_the_excluded_magnitude() {
    let note = unzoned_note(15_120).expect("a non-zero remainder is disclosed");
    assert!(note.starts_with("4h 12m"), "got: {note}");
}

#[test]
fn has_time_patterns_is_false_for_a_window_of_zeros() {
    // AC7: a period whose sessions could none of them be placed on a local
    // clock renders the card's empty state, not two rows of flat bars.
    let summary = StatsSummary {
        hour_of_day: hours([0; 24]),
        unzoned_seconds: 900,
        ..Default::default()
    };
    assert!(!summary.has_time_patterns());
}
