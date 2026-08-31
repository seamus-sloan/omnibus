//  StatsRedesignTests.swift
//  The Stats tab's derivations: the daily-goal wire contract, the year card's
//  projection, and the copy each empty state is supposed to produce.
//
//  Everything the redesign draws is derived rather than stored, so these are
//  the only place the arithmetic is pinned — a wrong projection or a peak hour
//  that reads "12am" on an empty window is a screen that looks right and lies.

import Foundation
import SwiftUI
import Testing

@testable import omnibus

// MARK: - Wire contract

@Suite("Daily goals decoding")
struct DailyGoalCodecTests {
    private func decode(_ json: String) throws -> DailyGoals {
        try JSONDecoder().decode(DailyGoals.self, from: Data(json.utf8))
    }

    @Test("both kinds decode with their own day")
    func decodesBothKinds() throws {
        // The days differ on purpose: minutes are bucketed on the reader's
        // local day and pages on the UTC day, so a single shared `day` on the
        // parent would have to be wrong for one of them.
        let goals = try decode(
            """
            {"pages":{"kind":"pages","target":250,"current":184,"day":"2026-08-30"},
             "minutes":{"kind":"minutes","target":150,"current":156,"day":"2026-08-29"},
             "unzoned_seconds":420}
            """)
        #expect(goals.pages?.target == 250)
        #expect(goals.pages?.day == "2026-08-30")
        #expect(goals.minutes?.current == 156)
        #expect(goals.minutes?.day == "2026-08-29")
        #expect(goals.unzonedSeconds == 420)
        #expect(!goals.isEmpty)
    }

    @Test("a cleared kind is an absent goal, never a zero target")
    func decodesOneKindSet() throws {
        let goals = try decode(
            """
            {"pages":null,"minutes":{"kind":"minutes","target":150,"current":20,"day":"2026-08-30"}}
            """)
        #expect(goals.pages == nil)
        #expect(goals[.minutes]?.target == 150)
        #expect(!goals.isEmpty, "one kind set is not an empty card")
    }

    @Test("a server with no daily goals at all still decodes")
    func decodesEmptyObject() throws {
        let goals = try decode("{}")
        #expect(goals.isEmpty)
        #expect(goals.unzonedSeconds == 0)
    }

    @Test("an over-target day reports the overshoot and clamps only the arc")
    func clampsArcNotRatio() {
        var goal = DailyGoal()
        goal.target = 150
        goal.current = 156
        #expect(goal.isMet)
        #expect(goal.fraction == 1, "the ring never wraps past its own start")
        #expect(goal.over == 6)
        #expect(goal.remaining == 0)
    }

    @Test("clearing a kind sends an explicit null target")
    func encodesNullTarget() throws {
        let body = try JSONEncoder().encode(DailyGoalUpdate(kind: .pages, target: nil))
        let json = String(decoding: body, as: UTF8.self)
        // Not merely an absent key: the server treats both as a clear, but the
        // null states the intent on the wire.
        #expect(json.contains("\"target\":null"))
        #expect(json.contains("\"kind\":\"pages\""))
    }

    @Test("per-kind bounds mirror MAX_DAILY_PAGES and MAX_DAILY_MINUTES")
    func boundsArePerKind() {
        #expect(DailyGoalKind.pages.maxTarget == 2_000)
        #expect(DailyGoalKind.minutes.maxTarget == 1_440)
    }
}

@Suite("Summary fields the redesign added")
struct StatsSummaryRedesignCodecTests {
    private func decode(_ json: String) throws -> StatsSummary {
        try JSONDecoder().decode(StatsSummary.self, from: Data(json.utf8))
    }

    @Test("the previous window and the genre denominator decode")
    func decodesComparisonAndTagged() throws {
        let summary = try decode(
            """
            {"range":"month","reading_seconds":600,"listening_seconds":0,"sessions":1,
             "active_days":2,"longest_streak_days":9,"busiest_week_seconds":0,
             "books_finished":4,"genre_tagged_books":7,"heatmap":[],"top_authors":[],
             "top_tags":[],"finished_books":[],
             "previous":{"books_finished":2,"avg_stars":4.1,"listening_seconds":30,
                         "pages_read":1088}}
            """)
        #expect(summary.genreTaggedBooks == 7)
        #expect(summary.previous.booksFinished == 2)
        #expect(summary.previous.pagesRead == 1088)
        #expect(summary.previous.avgStars == 4.1)
    }

    @Test("a server that sends neither still decodes the whole tab")
    func decodesWithoutThem() throws {
        // Both are `#[serde(default)]` on the Rust side; the app ships ahead
        // of a self-hosted server routinely, and a missing field must cost one
        // delta, not the screen.
        let summary = try decode(
            """
            {"range":"month","reading_seconds":600,"listening_seconds":0,"sessions":1,
             "active_days":2,"longest_streak_days":9,"busiest_week_seconds":0,
             "books_finished":4,"heatmap":[],"top_authors":[],"top_tags":[],
             "finished_books":[]}
            """)
        #expect(summary.genreTaggedBooks == 0)
        #expect(summary.previous.booksFinished == 0)
        #expect(summary.dailyGoals.isEmpty)
    }

    @Test("the pages ledger's per-day series decodes")
    func decodesPagesDaily() throws {
        let summary = try decode(
            """
            {"range":"month","reading_seconds":600,"listening_seconds":0,"sessions":1,
             "active_days":2,"longest_streak_days":9,"busiest_week_seconds":0,
             "books_finished":1,"heatmap":[],"top_authors":[],"top_tags":[],
             "finished_books":[],
             "pages_detail":{"since_day":"2026-01-01","measured_books":1,
                             "unmeasured_books":0,"audio_books":0,
                             "daily":[{"label":"2026-08-29","value":40},
                                      {"label":"2026-08-30","value":184}],
                             "window_predates_ledger":false}}
            """)
        #expect(summary.pagesDetail.daily.count == 2)
        #expect(summary.pagesDetail.daily.last?.value == 184)
    }
}

// MARK: - Today's figure with no target

@Suite("Daily goal card, no target")
struct TodaysFigureTests {
    private func summary() -> StatsSummary {
        var s = StatsSummary()
        s.asOfDay = "2026-08-30"
        s.pagesDetail.daily = [
            TrendPoint(label: "2026-08-29", value: 40),
            TrendPoint(label: "2026-08-30", value: 184),
        ]
        s.heatmap = [
            DayActivity(day: "2026-08-29", seconds: 3_600),
            DayActivity(day: "2026-08-30", seconds: 9_360),
        ]
        return s
    }

    @Test("today's pages come off the ledger's own day, not the last entry")
    func pagesReadOffAsOfDay() {
        // Keyed on `asOfDay` rather than taking the tail: the ledger carries
        // active days only, so a reader who hasn't opened a book today has
        // yesterday's figure sitting last in the array.
        #expect(DailyGoalsCard.todaysFigure(.pages, summary()) == 184)
    }

    @Test("today's minutes come off the heatmap, in whole minutes")
    func minutesFromHeatmap() {
        #expect(DailyGoalsCard.todaysFigure(.minutes, summary()) == 156)
    }

    @Test("a day with nothing recorded reports zero, not the previous day")
    func quietDayReportsZero() {
        var s = summary()
        s.asOfDay = "2026-08-31"
        #expect(DailyGoalsCard.todaysFigure(.pages, s) == 0)
        #expect(DailyGoalsCard.todaysFigure(.minutes, s) == 0)
    }
}

// MARK: - Year projection

@Suite("Year goal projection")
struct YearProjectionTests {
    /// Eight months of 2026 with a trailing-12 window that opens in 2025 — the
    /// shape the payload actually has for any month before December.
    private func summary(target: Int64? = nil) -> StatsSummary {
        var s = StatsSummary()
        s.asOfDay = "2026-08-30"
        s.booksPerMonth = [
            MonthCount(month: "2025-09", books: 9),
            MonthCount(month: "2025-10", books: 9),
            MonthCount(month: "2025-11", books: 9),
            MonthCount(month: "2025-12", books: 9),
            MonthCount(month: "2026-01", books: 3),
            MonthCount(month: "2026-02", books: 2),
            MonthCount(month: "2026-03", books: 4),
            MonthCount(month: "2026-04", books: 1),
            MonthCount(month: "2026-05", books: 2),
            MonthCount(month: "2026-06", books: 3),
            MonthCount(month: "2026-07", books: 0),
            MonthCount(month: "2026-08", books: 4),
        ]
        if let target {
            s.goal = ReadingGoal(kind: "books", target: target, current: 19, year: 2026)
        }
        return s
    }

    @Test("last year's months are excluded from this year's curve")
    func excludesPriorYear() {
        let p = YearProjection(summary: summary(), year: "2026")
        // 3+2+4+1+2+3+0+4 = 19. Taking `booksPerMonth` as sent would have
        // opened the curve on 36 books from 2025.
        #expect(p.finished == 19)
        #expect(p.cumulative.count == 8, "January through the current month")
        #expect(p.cumulative.first == 3)
        #expect(p.nowIndex == 7)
    }

    @Test("pace is days a book, and the projection carries it to year end")
    func projectsFromPace() {
        let p = YearProjection(summary: summary(), year: "2026")
        // 30 August is day 242 of 365; 242 / 19 books ≈ 12.7 days a book, and
        // 123 days remain.
        #expect(p.dayOfYear == 242)
        #expect(p.daysInYear == 365)
        #expect(p.daysLeft == 123)
        let pace = try! #require(p.daysPerBook)
        #expect(abs(pace - 12.7368) < 0.001)
        #expect(p.projected == 29)
    }

    @Test("a target adds the rate still needed, as days a book")
    func needsRateWithTarget() {
        let p = YearProjection(summary: summary(target: 30), year: "2026")
        #expect(p.remaining == 11)
        let needed = try! #require(p.neededDaysPerBook)
        #expect(abs(needed - 123.0 / 11.0) < 0.001)
        #expect(p.stats.count == 3)
        #expect(p.stats.last?.label == "days a book to hit 30")
    }

    @Test("with no target the card drops to two stats and no goal line")
    func twoStatsWithoutTarget() {
        let p = YearProjection(summary: summary(), year: "2026")
        #expect(p.target == nil)
        #expect(p.stats.count == 2)
        #expect(p.stats.map(\.id) == ["pace", "projected"])
    }

    @Test("a met target reports the tick rather than a needed rate")
    func metTargetReportsDone() {
        let p = YearProjection(summary: summary(target: 12), year: "2026")
        #expect(p.remaining == 0)
        #expect(p.neededDaysPerBook == nil)
        #expect(p.stats.last?.label == "goal already met")
    }

    @Test("a year with nothing finished has no pace and projects nothing")
    func emptyYearHasNoPace() {
        var s = StatsSummary()
        s.asOfDay = "2026-03-04"
        let p = YearProjection(summary: s, year: "2026")
        #expect(p.finished == 0)
        // Dividing the year so far by zero books would report an infinite
        // pace, which renders as "inf days a book".
        #expect(p.daysPerBook == nil)
        #expect(p.projected == 0)
        #expect(p.stats.first?.value == "\u{2014}")
    }
}

// MARK: - Windowed copy

@Suite("Window labels and deltas")
struct WindowCopyTests {
    private func summary(_ range: StatsRange) -> StatsSummary {
        var s = StatsSummary()
        s.range = range
        s.asOfDay = "2026-08-30"
        return s
    }

    @Test("each range names the window it actually covers")
    func captionsNameTheWindow() {
        // Week is a rolling seven days ending today, matching
        // `window_start_expr`'s `-6 days` — not a calendar week.
        #expect(StatsView.rangeCaption(summary(.week)) == "Week of 24 Aug")
        #expect(StatsView.rangeCaption(summary(.month)) == "August 2026")
        #expect(StatsView.rangeCaption(summary(.year)) == "2026 to date")
        #expect(StatsView.rangeCaption(summary(.allTime)) == "Everything recorded")
    }

    @Test("a server too old to send its day falls back to the range's name")
    func captionWithoutAsOfDay() {
        var s = StatsSummary()
        s.range = .week
        #expect(StatsView.rangeCaption(s) == "This week")
    }

    @Test("Lifetime is shortened for the control but not for the eyebrow")
    func shortLabels() {
        #expect(RangeControl.shortLabel(.allTime) == "All")
        #expect(RangeControl.shortLabel(.week) == "Week")
        #expect(StatsView.windowLabel(.allTime) == "All time")
    }

    @Test("deltas are signed, and absent when nothing moved")
    func signedDeltas() {
        #expect(StatsFormat.delta(4, 2) == "+2")
        #expect(StatsFormat.delta(1, 2) == "\u{2212}1")
        #expect(StatsFormat.delta(2, 2) == nil)
    }

    @Test("a percentage delta needs a baseline to be a percentage of")
    func percentDeltas() {
        #expect(StatsFormat.percentDelta(1284, 1088) == "+18%")
        #expect(StatsFormat.percentDelta(900, 1000) == "\u{2212}10%")
        // Not "+∞%", and not "+100%": there is no percentage change from zero.
        #expect(StatsFormat.percentDelta(500, 0) == nil)
    }
}

// MARK: - Reading clock

@Suite("Reading clock")
struct ReadingClockTests {
    private func summary(_ hours: [Int64]) -> StatsSummary {
        var s = StatsSummary()
        s.hourOfDay = hours.enumerated().map { HourBucket(hour: Int64($0.offset), seconds: $0.element) }
        return s
    }

    @Test("the peak hour reads as a clock does")
    func peakHourLabel() {
        var hours = [Int64](repeating: 0, count: 24)
        hours[20] = 68
        #expect(ReadingClock.peakLabel(summary(hours)) == "8pm")
        hours = [Int64](repeating: 0, count: 24)
        hours[0] = 5
        #expect(ReadingClock.peakLabel(summary(hours)) == "12am")
        hours = [Int64](repeating: 0, count: 24)
        hours[12] = 5
        #expect(ReadingClock.peakLabel(summary(hours)) == "12pm")
    }

    @Test("an empty window has no peak hour, and never midnight")
    func emptyWindowHasNoPeak() {
        // The naive answer — the first index of the maximum — is 0 for an
        // all-zero array, which renders a window with no activity as one whose
        // reader is up at midnight.
        #expect(ReadingClock.peakLabel(summary([Int64](repeating: 0, count: 24))) == "\u{2014}")
    }

    @Test("the line names the quarter of the day that carries the reading")
    func clockLineNamesTheBand() {
        var hours = [Int64](repeating: 0, count: 24)
        hours[19] = 30
        hours[20] = 50
        hours[9] = 20
        let line = ReadingClock.clockLine(summary(hours))
        #expect(line == "Evenings carry it \u{2014} 80% of your time falls after six.")
    }

    @Test("a window with nothing placeable says so rather than naming a band")
    func clockLineOnEmptyWindow() {
        let line = ReadingClock.clockLine(summary([Int64](repeating: 0, count: 24)))
        #expect(line == "Nothing placeable on a clock yet.")
    }
}

// MARK: - Genre donut

@Suite("Genre donut")
struct GenreDonutTests {
    @Test("the top four are named and the tail becomes one Other slice")
    func lumpsTheTail() {
        let share = [
            GenreShare(name: "Science fiction", books: 36),
            GenreShare(name: "History", books: 28),
            GenreShare(name: "Literary fiction", books: 18),
            GenreShare(name: "Essays", books: 11),
            GenreShare(name: "Poetry", books: 4),
            GenreShare(name: "Travel", books: 3),
        ]
        let slices = GenreDonut.slices(share, palette: .atrium)
        #expect(slices.count == 5)
        #expect(slices.map(\.name).last == "Other")
        // 7 of 100 assignments, both tail genres folded together.
        #expect(slices.last?.percent == 7)
    }

    @Test("a library with no genres assigned draws no ring at all")
    func emptyShareDrawsNothing() {
        #expect(GenreDonut.slices([], palette: .atrium).isEmpty)
    }

    @Test("stops are cumulative, so a zero slice collapses to no width")
    func cumulativeStops() {
        let slices = [
            GenreSlice(name: "A", percent: 60, color: .red),
            GenreSlice(name: "B", percent: 0, color: .green),
            GenreSlice(name: "C", percent: 40, color: .blue),
        ]
        let stops = GenreDonut.stops(slices)
        #expect(stops.count == 6)
        #expect(abs(stops[1].location - 0.6) < 0.0001)
        // B opens and closes at the same place: a hairline of colour between
        // its neighbours is what a non-cumulative gradient leaves behind.
        #expect(stops[2].location == stops[3].location)
        #expect(abs((stops.last?.location ?? 0) - 1.0) < 0.0001)
    }
}

// MARK: - Standing strips

@Suite("Standing strips")
struct StandingStripTests {
    @Test("the four-week sparkline fills the days the heatmap leaves out")
    func trailingFillsGaps() {
        var s = StatsSummary()
        s.asOfDay = "2026-08-30"
        // The heatmap carries active days only; drawn off those alone the
        // sparkline would silently close its own rest days up.
        s.heatmap = [
            DayActivity(day: "2026-08-30", seconds: 900),
            DayActivity(day: "2026-08-28", seconds: 600),
        ]
        let days = LastFourWeeksCard.trailing(28, of: s)
        #expect(days.count == 28)
        #expect(days.first?.day == "2026-08-03")
        #expect(days.last?.day == "2026-08-30")
        #expect(days.last?.seconds == 900)
        #expect(days[26].seconds == 0, "29 August is a rest day, not a missing column")
        #expect(days[25].seconds == 600)
    }

    @Test("a summary with no server day draws no sparkline rather than the device's")
    func trailingNeedsTheServersDay() {
        #expect(LastFourWeeksCard.trailing(28, of: StatsSummary()).isEmpty)
    }

    @Test("the trailing-12 columns are labelled by month initial")
    func monthInitials() {
        #expect(TrailingYearCard.initial("2026-01") == "J")
        #expect(TrailingYearCard.initial("2026-09") == "S")
        #expect(TrailingYearCard.initial("2026-12") == "D")
        #expect(TrailingYearCard.initial("nonsense") == "")
    }
}
