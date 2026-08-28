//! Unit tests for the stats DTO helpers and range serialization.

use super::*;

#[test]
fn total_seconds_sums_reading_and_listening() {
    let s = StatsSummary {
        reading_seconds: 120,
        listening_seconds: 300,
        ..Default::default()
    };
    assert_eq!(s.total_seconds(), 420);
}

#[test]
fn is_empty_is_true_only_without_recorded_time_or_finishes() {
    assert!(StatsSummary::default().is_empty());
    assert!(!StatsSummary {
        reading_seconds: 1,
        ..Default::default()
    }
    .is_empty());
    assert!(!StatsSummary {
        listening_seconds: 1,
        ..Default::default()
    }
    .is_empty());
    assert!(!StatsSummary {
        books_finished: 1,
        ..Default::default()
    }
    .is_empty());
}

#[test]
fn is_empty_is_false_for_recorded_time_that_no_sitting_qualified_for() {
    // `sessions` is filtered by the server's minimum sitting length, so a
    // reader with only glances has zero of them — but a month of time read,
    // active days and a heatmap to show. The page must not blank.
    let glances_only = StatsSummary {
        reading_seconds: 1650,
        sessions: 0,
        books_finished: 0,
        active_days: 30,
        ..Default::default()
    };
    assert!(!glances_only.is_empty());
}

#[test]
fn range_defaults_to_month_and_serializes_snake_case() {
    assert_eq!(StatsRange::default(), StatsRange::Month);
    assert_eq!(
        serde_json::to_string(&StatsRange::AllTime).unwrap(),
        "\"all_time\""
    );
    assert_eq!(
        serde_json::from_str::<StatsRange>("\"week\"").unwrap(),
        StatsRange::Week
    );
}

#[test]
fn avg_stars_defaults_to_none_when_absent_from_the_wire() {
    // Older payloads predate the field; serde(default) keeps them parseable.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.avg_stars, None);
}

#[test]
fn books_per_month_defaults_to_empty_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars — the monthly trend chart is a
    // newer field, so a pre-existing payload without it must still parse.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.books_per_month.is_empty());
}

#[test]
fn month_count_round_trips_through_json() {
    let m = MonthCount {
        month: "2026-07".to_string(),
        books: 3,
    };
    let wire = serde_json::to_string(&m).unwrap();
    assert_eq!(serde_json::from_str::<MonthCount>(&wire).unwrap(), m);
}

#[test]
fn previous_and_trend_fields_default_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/books_per_month — the drill-in
    // fields are newer, so a pre-existing payload without them must still parse.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.previous, PeriodComparison::default());
    assert!(s.listening_daily.is_empty());
    assert!(s.rating_monthly.is_empty());
}

#[test]
fn pages_read_defaults_to_none_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/books_per_month — the Pages
    // tile field is newer, so a pre-existing payload without it must
    // still parse.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.pages_read, None);
}

#[test]
fn finished_book_cover_url_and_rating_default_to_none_when_absent() {
    let b: FinishedBook =
        serde_json::from_str(r#"{"book_uuid":"u1","title":"Dune","author":null,"finished_at":0}"#)
            .unwrap();
    assert_eq!(b.cover_url, None);
    assert_eq!(b.rating, None);
}

#[test]
fn trend_point_round_trips_through_json() {
    let p = TrendPoint {
        label: "2026-07".to_string(),
        value: 4.25,
    };
    let wire = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<TrendPoint>(&wire).unwrap(), p);
}

#[test]
fn current_streak_days_defaults_to_zero_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/pages_read — the current streak
    // is a newer field, so a server that predates it must still parse. Zero is
    // the honest default too: absent means unknown, and "no streak" is the one
    // claim that can't overstate one.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":7,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.current_streak_days, 0);
    assert_eq!(s.longest_streak_days, 7, "the record still decodes");
}

#[test]
fn rating_bucket_round_trips_through_json() {
    let b = RatingBucket {
        half_stars: 7,
        books: 4,
    };
    let wire = serde_json::to_string(&b).unwrap();
    assert_eq!(wire, r#"{"half_stars":7,"books":4}"#);
    assert_eq!(serde_json::from_str::<RatingBucket>(&wire).unwrap(), b);
}

#[test]
fn rating_histogram_defaults_to_empty_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/pages_read. Empty is
    // distinguishable from "nothing was rated", which the server sends as ten
    // zero buckets — see the field's docs.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.rating_histogram.is_empty());
}

#[test]
fn length_bucket_round_trips_through_json() {
    let b = LengthBucket {
        label: "300\u{2013}499".to_string(),
        books: 2,
    };
    let wire = serde_json::to_string(&b).unwrap();
    assert_eq!(serde_json::from_str::<LengthBucket>(&wire).unwrap(), b);
}

#[test]
fn length_buckets_default_to_empty_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/pages_read.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.length_buckets.is_empty());
}

#[test]
fn book_superlative_round_trips_through_json() {
    let b = BookSuperlative {
        book_uuid: "0f2c-uuid".to_string(),
        title: "Doorstopper".to_string(),
        author: Some("Ursula K. Le Guin".to_string()),
        value: 900,
    };
    let wire = serde_json::to_string(&b).unwrap();
    assert_eq!(serde_json::from_str::<BookSuperlative>(&wire).unwrap(), b);
}

#[test]
fn book_superlative_author_survives_a_null_on_the_wire() {
    // The db left-joins the author, so a book with no position-0 creator
    // still wins its category and arrives with an explicit null.
    let b: BookSuperlative =
        serde_json::from_str(r#"{"book_uuid":"u","title":"Untitled","author":null,"value":12}"#)
            .unwrap();
    assert_eq!(b.author, None);
}

#[test]
fn superlatives_is_empty_only_when_every_figure_is_absent() {
    let mut s = Superlatives::default();
    assert!(s.is_empty());

    s.biggest_day = Some(DayActivity {
        day: "2023-11-15".to_string(),
        seconds: 3600,
    });
    assert!(!s.is_empty());
}

#[test]
fn superlatives_default_to_empty_when_absent_from_the_wire() {
    // An app running ahead of its server must lose the card, not the tab.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.superlatives.is_empty());
}

#[test]
fn one_absent_superlative_costs_only_its_own_field() {
    // Each field carries its own `#[serde(default)]`, so a server that omits
    // a single figure must not blank the four beside it.
    let s: Superlatives = serde_json::from_str(
        r#"{"longest_book":{"book_uuid":"u","title":"T","author":null,"value":900}}"#,
    )
    .unwrap();
    assert_eq!(s.longest_book.map(|b| b.value), Some(900));
    assert!(s.shortest_book.is_none());
    assert!(s.fastest_read.is_none());
}

#[test]
fn as_query_matches_the_serde_wire_name() {
    for range in StatsRange::ALL {
        let wire = serde_json::to_string(&range).unwrap();
        assert_eq!(wire, format!("\"{}\"", range.as_query()));
    }
}

#[test]
fn range_labels_render_all_time_as_lifetime() {
    let labels: Vec<&str> = StatsRange::ALL.iter().map(|r| r.label()).collect();
    assert_eq!(labels, ["Week", "Month", "Year", "Lifetime"]);
}

#[test]
fn measured_total_is_empty_only_without_a_book_behind_it() {
    // `books`, not `total`, is the emptiness test: a library whose only
    // measured audiobook is twenty minutes long has a real figure to show.
    let measured_but_short = MeasuredTotal { total: 0, books: 1 };
    let total_without_a_denominator = MeasuredTotal {
        total: 412,
        books: 0,
    };

    assert!(MeasuredTotal::default().is_empty());
    assert!(!measured_but_short.is_empty());
    assert!(total_without_a_denominator.is_empty());
}

#[test]
fn library_size_is_empty_only_when_nothing_at_all_is_measured() {
    let words_only = LibrarySize {
        books: 3,
        words: MeasuredTotal {
            total: 275,
            books: 1,
        },
        ..Default::default()
    };

    assert!(LibrarySize::default().is_empty());
    assert!(!words_only.is_empty());
}

#[test]
fn library_size_measurements_default_when_absent_from_the_wire() {
    // Same older-server contract the iOS decoder relies on: a server that
    // predates these fields still yields a decodable, empty LibrarySize.
    let size: LibrarySize = serde_json::from_str(r#"{"books":1510}"#).unwrap();
    assert_eq!(size.books, 1510);
    assert!(size.words.is_empty());
    assert!(size.pages.is_empty());
    assert!(size.listening_seconds.is_empty());
    assert!(size.is_empty());
}

#[test]
fn library_size_round_trips_its_totals_with_their_coverage() {
    let size = LibrarySize {
        books: 1510,
        words: MeasuredTotal {
            total: 412_000_000,
            books: 1204,
        },
        pages: MeasuredTotal {
            total: 1_600_000,
            books: 1204,
        },
        listening_seconds: MeasuredTotal {
            total: 8_121_600,
            books: 96,
        },
    };

    let wire = serde_json::to_string(&size).unwrap();

    // The denominator has to survive the wire — a total that arrived without
    // one is the confidently-wrong number this type exists to prevent.
    assert!(wire.contains(r#""listening_seconds":{"total":8121600,"books":96}"#));
    assert_eq!(serde_json::from_str::<LibrarySize>(&wire).unwrap(), size);
}

#[test]
fn has_time_patterns_is_true_only_when_some_local_hour_carries_activity() {
    let mut s = StatsSummary {
        hour_of_day: (0..24)
            .map(|hour| HourBucket { hour, seconds: 0 })
            .collect(),
        ..Default::default()
    };
    // Zero-filled to 24 either way, so the count can't be the predicate.
    assert!(!s.has_time_patterns());
    s.hour_of_day[21].seconds = 600;
    assert!(s.has_time_patterns());
}

#[test]
fn time_pattern_fields_default_on_a_payload_from_an_older_server() {
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.hour_of_day.is_empty());
    assert!(s.day_of_week.is_empty());
    assert_eq!(s.unzoned_seconds, 0);
    assert!(!s.has_time_patterns());
}

#[test]
fn goal_defaults_to_none_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/pages_read: a server that
    // predates the goal must still decode into a whole summary.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.goal.is_none());
}

#[test]
fn reading_goal_round_trips_through_the_wire() {
    let goal = ReadingGoal {
        kind: GOAL_KIND_BOOKS.to_string(),
        target: 24,
        current: 7,
        year: 2026,
    };
    let wire = serde_json::to_string(&goal).unwrap();
    assert_eq!(serde_json::from_str::<ReadingGoal>(&wire).unwrap(), goal);
}

#[test]
fn reading_goal_percent_clamps_the_bar_while_the_ratio_stays_honest() {
    let goal = |current, target| ReadingGoal {
        kind: GOAL_KIND_BOOKS.to_string(),
        target,
        current,
        year: 2026,
    };
    assert_eq!(goal(0, 24).percent(), 0);
    assert_eq!(goal(12, 24).percent(), 50);
    // Past the target: the bar caps at full, `current` does not.
    let over = goal(30, 24);
    assert_eq!(over.percent(), 100);
    assert_eq!(over.current, 30);
    assert!(over.is_met());
    assert_eq!(over.remaining(), 0);
    assert_eq!(goal(20, 24).remaining(), 4);
    assert!(!goal(20, 24).is_met());
}

#[test]
fn reading_goal_update_defaults_to_the_books_kind_and_the_servers_year() {
    let update = ReadingGoalUpdate::books(24);
    assert_eq!(update.kind_or_default(), GOAL_KIND_BOOKS);
    assert_eq!(update.year, None, "the server owns the calendar year");
    assert_eq!(update.target, Some(24));

    let cleared = ReadingGoalUpdate::clear_books();
    assert_eq!(cleared.target, None);
    assert_eq!(cleared.kind_or_default(), GOAL_KIND_BOOKS);
}

#[test]
fn reading_goal_update_validate_accepts_in_range_values_and_rejects_the_rest() {
    assert!(ReadingGoalUpdate::books(1).validate().is_ok());
    assert!(ReadingGoalUpdate::books(MAX_GOAL_TARGET).validate().is_ok());
    assert!(ReadingGoalUpdate::clear_books().validate().is_ok());

    assert!(ReadingGoalUpdate::books(0).validate().is_err());
    assert!(ReadingGoalUpdate::books(MAX_GOAL_TARGET + 1)
        .validate()
        .is_err());
    assert!(ReadingGoalUpdate {
        kind: Some("pages".to_string()),
        target: Some(500),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(ReadingGoalUpdate {
        year: Some(MIN_GOAL_YEAR - 1),
        target: Some(12),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(ReadingGoalUpdate {
        year: Some(MAX_GOAL_YEAR + 1),
        target: Some(12),
        ..Default::default()
    }
    .validate()
    .is_err());
}

/// A `target` that is present but absent-shaped on the wire (`null`) must
/// read as a clear, not as a decode failure — the two clients send it.
#[test]
fn reading_goal_update_decodes_a_bare_target_and_an_explicit_null() {
    let bare: ReadingGoalUpdate = serde_json::from_str(r#"{"target":24}"#).unwrap();
    assert_eq!(bare, ReadingGoalUpdate::books(24));

    let cleared: ReadingGoalUpdate = serde_json::from_str(r#"{"target":null}"#).unwrap();
    assert_eq!(cleared, ReadingGoalUpdate::clear_books());

    let empty: ReadingGoalUpdate = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, ReadingGoalUpdate::clear_books());
}

#[test]
fn reading_goal_update_sends_only_the_fields_it_names() {
    // The two clients' bodies are asserted verbatim by the E2E spec, so the
    // wire shape is a contract: a set is `{"target":N}` and a clear is `{}`.
    assert_eq!(
        serde_json::to_string(&ReadingGoalUpdate::books(24)).unwrap(),
        r#"{"target":24}"#
    );
    assert_eq!(
        serde_json::to_string(&ReadingGoalUpdate::clear_books()).unwrap(),
        "{}"
    );
}

#[test]
fn pages_read_detail_audio_only_requires_no_reading_at_all() {
    let listening = PagesReadDetail {
        audio_books: 2,
        ..Default::default()
    };
    assert!(listening.audio_only());
    // Any reading in the window — measurable or not — means the em-dash is the
    // honest answer, because something happened that pages describe.
    assert!(!PagesReadDetail {
        audio_books: 2,
        unmeasured_books: 1,
        ..Default::default()
    }
    .audio_only());
    assert!(!PagesReadDetail {
        audio_books: 2,
        measured_books: 1,
        ..Default::default()
    }
    .audio_only());
    // An empty window is not audio-only; it is empty.
    assert!(!PagesReadDetail::default().audio_only());
}

#[test]
fn pages_read_detail_predates_ledger_only_for_ranges_that_reach_back() {
    let detail = PagesReadDetail {
        since_day: Some("2026-08-01".to_string()),
        ..Default::default()
    };
    assert!(detail.predates_ledger(StatsRange::AllTime));
    assert!(detail.predates_ledger(StatsRange::Year));
    assert!(!detail.predates_ledger(StatsRange::Month));
    assert!(!detail.predates_ledger(StatsRange::Week));
    // Nothing recorded, nothing to disclose.
    assert!(!PagesReadDetail::default().predates_ledger(StatsRange::AllTime));
}

#[test]
fn stats_summary_decodes_a_payload_from_a_server_without_the_pages_detail() {
    // The app ships ahead of a self-hosted server routinely; a stats field it
    // hasn't learned yet must cost one tile, not the whole screen.
    let json = r#"{"range":"month","reading_seconds":60,"listening_seconds":0,
        "avg_stars":null,"sessions":1,"active_days":1,"longest_streak_days":1,
        "busiest_week_start":null,"busiest_week_seconds":0,"books_finished":0,
        "heatmap":[],"top_authors":[],"top_tags":[],"finished_books":[],
        "pages_read":12}"#;
    let summary: StatsSummary = serde_json::from_str(json).unwrap();

    assert_eq!(summary.pages_read, Some(12));
    assert_eq!(summary.pages_detail, PagesReadDetail::default());
    assert_eq!(summary.previous.pages_read, 0);
}

#[test]
fn pages_read_detail_round_trips_over_the_wire() {
    let detail = PagesReadDetail {
        since_day: Some("2026-08-01".to_string()),
        measured_books: 3,
        unmeasured_books: 1,
        audio_books: 2,
        daily: vec![TrendPoint {
            label: "2026-08-01".to_string(),
            value: 41.0,
        }],
    };
    let json = serde_json::to_string(&detail).unwrap();
    assert_eq!(
        serde_json::from_str::<PagesReadDetail>(&json).unwrap(),
        detail
    );
    // Snake-case on the wire, as every other field on the summary is.
    assert!(json.contains("\"since_day\""), "{json}");
    assert!(json.contains("\"unmeasured_books\""), "{json}");
}
