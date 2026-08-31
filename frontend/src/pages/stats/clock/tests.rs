use super::*;

fn hours(values: [i64; 24]) -> Vec<HourBucket> {
    values
        .into_iter()
        .enumerate()
        .map(|(hour, seconds)| HourBucket {
            hour: i64::try_from(hour).unwrap_or(0),
            seconds,
        })
        .collect()
}

fn weekdays(values: [i64; 7]) -> Vec<WeekdayBucket> {
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        .into_iter()
        .zip(values)
        .enumerate()
        .map(|(i, (label, seconds))| WeekdayBucket {
            weekday: i64::try_from(i).unwrap_or(0),
            label: label.to_string(),
            seconds,
        })
        .collect()
}

#[test]
fn tick_transform_puts_midnight_at_the_top_and_walks_clockwise() {
    // The tick grows outward (downward) from the centre, so hour 0 needs the
    // half-turn to point up. Six hours on is a quarter turn from there.
    assert_eq!(tick_transform(0), "rotate(180deg) translateY(58px)");
    assert_eq!(tick_transform(6), "rotate(270deg) translateY(58px)");
    assert_eq!(tick_transform(12), "rotate(360deg) translateY(58px)");
    assert_eq!(tick_transform(18), "rotate(450deg) translateY(58px)");
}

#[test]
fn build_ticks_scales_to_the_busiest_hour_and_keeps_a_floor() {
    let mut values = [0i64; 24];
    values[20] = 1_000;
    values[8] = 500;
    values[3] = 10;
    let ticks = build_ticks(&hours(values));

    assert_eq!(ticks.len(), 24);
    assert_eq!(ticks[20].height_px, TICK_REACH_PX, "the full reach");
    assert_eq!(ticks[8].height_px, TICK_REACH_PX / 2);
    // A quiet-but-real hour still leaves a mark, and so does an empty one —
    // an invisible tick would read as a missing hour rather than a quiet one.
    assert_eq!(ticks[3].height_px, TICK_MIN_PX);
    assert_eq!(ticks[0].height_px, TICK_MIN_PX);
}

#[test]
fn build_ticks_weights_the_rim_so_it_reads_as_a_shape() {
    let mut values = [0i64; 24];
    values[20] = 100;
    values[19] = 50;
    values[8] = 20;
    values[3] = 1;
    let ticks = build_ticks(&hours(values));
    assert_eq!(ticks[20].weight, "hot");
    assert_eq!(ticks[19].weight, "warm");
    assert_eq!(ticks[8].weight, "cool");
    assert_eq!(ticks[3].weight, "idle");
    assert_eq!(ticks[0].weight, "idle");
}

#[test]
fn hour_label_reads_as_a_clock_not_as_an_index() {
    assert_eq!(hour_label(0), "12am");
    assert_eq!(hour_label(1), "1am");
    assert_eq!(hour_label(11), "11am");
    assert_eq!(hour_label(12), "12pm");
    assert_eq!(hour_label(20), "8pm");
    assert_eq!(hour_label(23), "11pm");
}

#[test]
fn peak_hour_names_the_busiest_hour_and_never_claims_midnight_on_an_empty_window() {
    let mut values = [0i64; 24];
    values[20] = 900;
    assert_eq!(peak_hour(&hours(values)), "8pm");
    // Taking the first maximum of a row of zeros would land on index 0 and
    // report that a reader with no activity reads at midnight.
    assert_eq!(peak_hour(&hours([0; 24])), "\u{2014}");
    assert_eq!(peak_hour(&[]), "\u{2014}");
}

#[test]
fn clock_line_names_the_part_of_the_day_the_window_belongs_to() {
    let mut evening = [0i64; 24];
    evening[19] = 300;
    evening[20] = 400;
    evening[21] = 300;
    assert_eq!(
        clock_line(&hours(evening)).as_deref(),
        Some("Evenings are yours \u{2014} 100% of your recorded time lands between 5pm and 10pm.")
    );

    // The late band wraps past midnight, so 11pm and 2am count together.
    let mut late = [0i64; 24];
    late[23] = 600;
    late[2] = 400;
    assert!(clock_line(&hours(late))
        .expect("a line")
        .starts_with("You read late"));
}

#[test]
fn clock_line_is_absent_when_nothing_can_be_placed_on_a_clock() {
    assert_eq!(clock_line(&hours([0; 24])), None);
    assert_eq!(clock_line(&[]), None);
}

#[test]
fn day_readout_never_reports_a_quiet_day_as_a_measured_zero() {
    assert_eq!(day_readout(0), "\u{2014}");
    assert_eq!(day_readout(58 * 60), "58m");
    assert_eq!(day_readout(3_600), "1h");
    assert_eq!(day_readout(3_600 + 28 * 60), "1h 28m");
}

#[test]
fn build_weekdays_scales_to_the_busiest_day_and_flags_the_quiet_ones() {
    let rows = build_weekdays(&weekdays([600, 0, 300, 0, 0, 0, 1_200]));
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[6].width_pct, 100);
    assert_eq!(rows[6].weight, "hot");
    assert_eq!(rows[0].width_pct, 50);
    assert_eq!(rows[0].weight, "warm");
    assert_eq!(rows[1].width_pct, 0);
    assert_eq!(rows[1].weight, "idle");
    assert_eq!(rows[1].readout, "\u{2014}");
}

#[test]
fn unzoned_note_discloses_only_what_it_could_not_place() {
    assert_eq!(unzoned_note(0), None);
    let note = unzoned_note(7 * 60).expect("a disclosure");
    assert!(note.starts_with("7m of activity"), "{note}");
}
