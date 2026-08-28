//  StatsCodecTests.swift
//  The Stats tab's wire contract with `omnibus_shared::StatsSummary`.
//
//  These models are hand-mirrored from Rust, so drift is silent — a renamed or
//  mistyped CodingKey doesn't fail to build, it just decodes to a default and
//  the tile reads zero forever.

import Foundation
import Testing

@testable import omnibus

/// A `StatsSummary` payload carrying every key the server currently sends.
/// Individual tests drop keys from it to pin the older-server contract.
private func summaryJSON(extra: String = "") -> String {
    """
    {"range":"month","reading_seconds":600,"listening_seconds":0,"avg_stars":4.5,
     "sessions":1,"active_days":2,"longest_streak_days":9,"current_streak_days":3,
     "busiest_week_start":null,"busiest_week_seconds":600,"books_finished":1,
     "books_active":1,"as_of_day":"2026-07-12","heatmap":[],"top_authors":[],
     "top_tags":[],"genre_share":[],"finished_books":[],"books_per_month":[],
     "pages_read":12\(extra)}
    """
}

private func decodeSummary(_ json: String) throws -> StatsSummary {
    try JSONDecoder().decode(StatsSummary.self, from: Data(json.utf8))
}

@Suite("Stats summary decoding")
struct StatsSummaryCodecTests {
    @Test("current and longest streak decode into their own fields")
    func decodesBothStreaks() throws {
        let summary = try decodeSummary(summaryJSON())
        // Distinct values on purpose: equal ones would let a swapped CodingKey
        // through, and the tab renders the two side by side.
        #expect(summary.currentStreakDays == 3)
        #expect(summary.longestStreakDays == 9)
    }

    @Test("a server that predates the current streak still decodes")
    func decodesWithoutCurrentStreak() throws {
        // The Rust field is `#[serde(default)]`, so the app must not hard-fail
        // the whole Stats tab against a server that hasn't been upgraded yet.
        let older = summaryJSON().replacingOccurrences(
            of: "\"current_streak_days\":3,", with: "")
        let summary = try decodeSummary(older)
        #expect(summary.currentStreakDays == 0)
        #expect(summary.longestStreakDays == 9, "the record still decodes")
    }

    @Test("the rating histogram decodes its half-star buckets in order")
    func decodesRatingHistogram() throws {
        let json = summaryJSON(
            extra: #","rating_histogram":[{"half_stars":1,"books":0},{"half_stars":7,"books":2}]"#
        )
        let summary = try decodeSummary(json)
        #expect(summary.ratingHistogram.count == 2)
        #expect(summary.ratingHistogram.first?.halfStars == 1)
        #expect(summary.ratingHistogram.last?.books == 2)
    }

    @Test("a server that predates the rating histogram still decodes")
    func decodesWithoutRatingHistogram() throws {
        let summary = try decodeSummary(summaryJSON())
        #expect(summary.ratingHistogram.isEmpty)
    }

    @Test("length buckets decode with their server-owned labels intact")
    func decodesLengthBuckets() throws {
        // The labels are the server's, not the client's — nothing here
        // re-derives a page range, so they have to survive the wire verbatim.
        let extra =
            #","length_buckets":[{"label":"Under 300","books":3},{"label":"Unknown","books":1}]"#
        let summary = try decodeSummary(summaryJSON(extra: extra))
        #expect(summary.lengthBuckets.count == 2)
        #expect(summary.lengthBuckets.first?.label == "Under 300")
        #expect(summary.lengthBuckets.first?.books == 3)
        #expect(summary.lengthBuckets.last?.label == "Unknown")
    }

    @Test("a server that predates the length distribution still decodes")
    func decodesWithoutLengthBuckets() throws {
        let summary = try decodeSummary(summaryJSON())
        #expect(summary.lengthBuckets.isEmpty)
    }

    @Test("the reading rate decodes as a fraction, not a truncated count")
    func decodesPagesPerHour() throws {
        // A fractional value on purpose: an Int64 CodingKey here would either
        // throw or silently floor 32.6 to 32.
        let summary = try decodeSummary(summaryJSON(extra: #","pages_per_hour":32.6"#))
        #expect(summary.pagesPerHour == 32.6)
    }

    @Test("a server that predates the reading rate still decodes")
    func decodesWithoutPagesPerHour() throws {
        // `nil` is the tile's em-dash, and must not be reachable as a zero —
        // "0 pages an hour" is a claim about the reader, not about the server.
        let summary = try decodeSummary(summaryJSON())
        #expect(summary.pagesPerHour == nil)
    }

    @Test("the time-pattern strips decode with the server's own bucket order")
    func decodesTimePatterns() throws {
        // Both are server-owned: the hour is already local (bucketed against
        // the offset each session recorded) and the weekday carries its label
        // so nothing here decides where the week starts.
        let extra =
            #","hour_of_day":[{"hour":0,"seconds":0},{"hour":21,"seconds":900}],"#
            + #""day_of_week":[{"weekday":0,"label":"Mon","seconds":0},"#
            + #"{"weekday":6,"label":"Sun","seconds":900}],"unzoned_seconds":1200"#
        let summary = try decodeSummary(summaryJSON(extra: extra))
        #expect(summary.hourOfDay.count == 2)
        #expect(summary.hourOfDay.last?.hour == 21)
        #expect(summary.hourOfDay.last?.seconds == 900)
        #expect(summary.dayOfWeek.first?.label == "Mon")
        #expect(summary.dayOfWeek.last?.weekday == 6)
        #expect(summary.unzonedSeconds == 1200)
        #expect(summary.hasTimePatterns)
    }

    @Test("a server that predates the time-pattern strips still decodes")
    func decodesWithoutTimePatterns() throws {
        let summary = try decodeSummary(summaryJSON())
        #expect(summary.hourOfDay.isEmpty)
        #expect(summary.dayOfWeek.isEmpty)
        #expect(summary.unzonedSeconds == 0)
        // The section is hidden rather than drawn as flat bars, and an older
        // server must land on that path rather than on an error state.
        #expect(!summary.hasTimePatterns)
    }

    @Test("an all-zero hour strip reads as no pattern, not as a flat day")
    func zeroFilledStripIsNotAPattern() throws {
        let extra =
            #","hour_of_day":[{"hour":0,"seconds":0},{"hour":1,"seconds":0}],"unzoned_seconds":600"#
        let summary = try decodeSummary(summaryJSON(extra: extra))
        #expect(!summary.hasTimePatterns)
        #expect(summary.unzonedSeconds == 600, "the excluded time is still disclosed")
    }
    @Test("superlatives decode, and an omitted one stays omitted")
    func decodesSuperlatives() throws {
        let longest = #"{"book_uuid":"u1","title":"Doorstopper","author":"A. Writer","value":900}"#
        let day = #"{"day":"2023-11-14","seconds":7200}"#
        let extra = #","superlatives":{"longest_book":\#(longest),"biggest_day":\#(day)}"#
        let summary = try decodeSummary(summaryJSON(extra: extra))
        #expect(summary.superlatives.longestBook?.title == "Doorstopper")
        #expect(summary.superlatives.longestBook?.value == 900)
        #expect(summary.superlatives.biggestDay?.seconds == 7200)
        // An absent superlative is `nil`, never a zero — a zero would render
        // as a finding.
        #expect(summary.superlatives.shortestBook == nil)
        #expect(summary.superlatives.fastestRead == nil)
        #expect(!summary.superlatives.isEmpty)
    }

    @Test("a server that predates superlatives still decodes")
    func decodesWithoutSuperlatives() throws {
        let summary = try decodeSummary(summaryJSON())
        #expect(summary.superlatives.isEmpty)
    }
}

@Suite("Session report timezone capture")
struct SessionReportZoneTests {
    @Test("a report stamps the device's current offset in whole minutes")
    func stampsLocalOffset() {
        let report = SessionReport(
            bookUUID: "uuid", format: .epub, startedAt: 0, endedAt: 60, progressUnits: 60,
            deviceId: nil)
        #expect(report.utcOffsetMinutes == SessionReport.localOffsetMinutes())
        // Minutes *east* of UTC, pinned against fixed zones rather than
        // re-derived: Los Angeles is -420 and Kolkata 330, and a flipped sign
        // would put a Los Angeles evening at 04:00 — the exact misreport this
        // field exists to prevent.
        #expect(SessionReport.localOffsetMinutes(in: TimeZone(secondsFromGMT: -25200)!) == -420)
        #expect(SessionReport.localOffsetMinutes(in: TimeZone(secondsFromGMT: 19800)!) == 330)
    }

    @Test("the offset rides the wire under its snake_case key")
    func encodesOffsetKey() throws {
        var report = SessionReport(
            bookUUID: "uuid", format: .epub, startedAt: 0, endedAt: 60, progressUnits: 60,
            deviceId: nil)
        report.utcOffsetMinutes = -420
        let json = try JSONSerialization.jsonObject(
            with: try JSONEncoder().encode(report)) as? [String: Any]
        #expect(json?["utc_offset_minutes"] as? Int == -420)
    }
}

@Suite("Standout rows")
struct StandoutRowTests {
    private func book(_ title: String, _ value: Int64) -> BookSuperlative {
        BookSuperlative(bookUUID: title, title: title, author: nil, value: value)
    }

    @Test("only the superlatives the window supports become rows")
    func omitsAbsentSuperlatives() throws {
        var summary = StatsSummary()
        summary.superlatives.longestBook = book("Doorstopper", 900)
        summary.superlatives.fastestRead = book("Sprint", 3)

        let rows = StatsView.standoutRows(summary)

        #expect(rows.map(\.label) == ["Longest book", "Fastest read"])
        // `#expect` records and continues, so a dropped row would leave the
        // indexed reads below to trap the whole test process rather than fail.
        try #require(rows.count == 2)
        #expect(rows[0].detail == "900 pages")
        #expect(rows[1].detail == "in 3 days")
    }

    @Test("a bare window produces no rows at all")
    func emptyWindowHasNoRows() {
        #expect(StatsView.standoutRows(StatsSummary()).isEmpty)
    }

    @Test("the busiest week needs seconds, not just a date")
    func busiestWeekNeedsSeconds() {
        // The field rides on every payload and is zeroed for an empty window;
        // rendering it off the date alone claims a week that never happened.
        var summary = StatsSummary()
        summary.busiestWeekStart = "2023-11-13"
        #expect(StatsView.standoutRows(summary).isEmpty)

        summary.busiestWeekSeconds = 14_400
        let rows = StatsView.standoutRows(summary)
        #expect(rows.first?.headline == "Week of 13 Nov 2023")
        #expect(rows.first?.detail == "4h")
    }

    @Test("day and count details read as sentences, not as raw numbers")
    func detailsSingularize() {
        #expect(StatsView.pagesDetail(1) == "1 page")
        #expect(StatsView.pagesDetail(412) == "412 pages")
        #expect(StatsView.daysDetail(1) == "in a day")
        #expect(StatsView.daysDetail(3) == "in 3 days")
        #expect(StatsView.prettyDay("2023-11-14") == "14 Nov 2023")
        #expect(StatsView.prettyDay("not-a-day") == "not-a-day")
    }
}

@Suite("Rating bucket labelling")
struct RatingBucketTests {
    @Test("buckets label themselves in stars, never in the stored half-stars")
    func labelsInStars() {
        // The wire scale is 1...10. Labelling it raw would present the chart as
        // a ten-point scale, which is the one way this axis can lie.
        let label = { (half: Int64) in RatingBucket(halfStars: half, books: 0).starLabel }
        #expect(label(1) == "0.5")
        #expect(label(2) == "1")
        #expect(label(7) == "3.5")
        #expect(label(10) == "5")
    }

    @Test("the axis filter keeps whole stars and drops the half steps")
    func wholeStarsOnly() {
        // `StatsView` hides any label containing "." so ten of them don't crowd
        // a phone's width — the half-star bars still draw, unlabelled.
        let kept = (1...10)
            .map { RatingBucket(halfStars: Int64($0), books: 0).starLabel }
            .filter { !$0.contains(".") }
        #expect(kept == ["1", "2", "3", "4", "5"])
    }
}

@Suite("Library size")
struct LibrarySizeTests {
    private func decodeSize(_ json: String) throws -> LibrarySize {
        try JSONDecoder().decode(LibrarySize.self, from: Data(json.utf8))
    }

    @Test("each total decodes with the coverage behind it")
    func decodesTotalsAndCoverage() throws {
        let json = """
            {"books":1510,"words":{"total":412000000,"books":1204},
             "pages":{"total":1600000,"books":1204},
             "listening_seconds":{"total":8121600,"books":88}}
            """
        let size = try decodeSize(json)
        #expect(size.books == 1510)
        #expect(size.words.total == 412_000_000)
        #expect(size.words.books == 1204)
        #expect(size.listeningSeconds.books == 88)
        #expect(!size.isEmpty)
    }

    @Test("a server that predates the totals still decodes to an empty size")
    func decodesWithoutTotals() throws {
        let size = try decodeSize(#"{"books":40}"#)
        #expect(size.books == 40)
        // Measured for nothing is an absent section, never three zeroes.
        #expect(size.isEmpty)
    }

    @Test("only measured figures become rows")
    func figuresSkipUnmeasured() {
        var size = LibrarySize()
        size.books = 1510
        size.words = MeasuredTotal(total: 412_000_000, books: 1204)

        let figures = StatsView.libraryFigures(size)

        #expect(figures.count == 1)
        #expect(figures[0].value == "412M")
        #expect(figures[0].unit == "words")
        // Built with the same formatter the label uses: the grouping separator
        // is locale-dependent, and pinning a comma would fail on a simulator
        // set to anything but a US locale. What this still catches — and is
        // here for — is the numerator and denominator being swapped.
        let grouped = { (n: Int64) in
            NumberFormatter.localizedString(from: NSNumber(value: n), number: .decimal)
        }
        #expect(figures[0].coverage == "across \(grouped(1204)) of \(grouped(1510)) books")
    }

    @Test("counts compact and audio picks the unit that fits it")
    func formatsLargeFigures() {
        #expect(StatsView.compactCount(812) == "812")
        #expect(StatsView.compactCount(94_200) == "94.2K")
        #expect(StatsView.compactCount(412_000_000) == "412M")
        // Promoted rather than rounded to "1000K" / "1000M" in the tier below.
        #expect(StatsView.compactCount(999_999) == "1.0M")
        #expect(StatsView.compactCount(999_999_999) == "1.0B")
        #expect(StatsView.compactCount(999_499) == "999K")
        #expect(StatsView.audioValue(3600) == ("1", "hour"))
        #expect(StatsView.audioValue(12 * 3600) == ("12", "hours"))
        #expect(StatsView.audioValue(94 * 24 * 3600) == ("94", "days"))
        // The unit follows the rounded figure, not the fraction behind it:
        // 1h40m is "2 hours", never "2 hour".
        #expect(StatsView.audioValue(6000) == ("2", "hours"))
        #expect(StatsView.audioValue(3500) == ("1", "hour"))
        #expect(StatsView.audioValue(167 * 3600 + 2400) == ("7", "days"))
    }
}

@Suite("Annual reading goal")
struct ReadingGoalCodecTests {
    @Test("the goal decodes off the summary with its year and progress")
    func decodesGoal() throws {
        let extra = #","goal":{"kind":"books","target":24,"current":7,"year":2026}"#
        let summary = try decodeSummary(summaryJSON(extra: extra))
        let goal = try #require(summary.goal)
        #expect(goal.kind == "books")
        #expect(goal.target == 24)
        #expect(goal.current == 7)
        #expect(goal.year == 2026)
    }

    @Test("a server that predates the goal still decodes, with no goal set")
    func decodesWithoutGoal() throws {
        // The whole point of `#[serde(default)] Option<ReadingGoal>` on the
        // Rust side: an older server costs one card, not the screen.
        let summary = try decodeSummary(summaryJSON())
        #expect(summary.goal == nil)
    }

    @Test("progress reports the honest ratio while the bar's fraction clamps")
    func clampsTheBarNotTheCount() {
        let over = ReadingGoal(kind: "books", target: 24, current: 30, year: 2026)
        #expect(over.fraction == 1)
        #expect(over.current == 30)
        #expect(over.isMet)
        #expect(over.remaining == 0)

        let partway = ReadingGoal(kind: "books", target: 24, current: 12, year: 2026)
        #expect(partway.fraction == 0.5)
        #expect(!partway.isMet)
        #expect(partway.remaining == 12)

        // A zero target never reaches a client, but the ring must not divide
        // by it if one ever does.
        #expect(ReadingGoal(kind: "books", target: 0, current: 3, year: 2026).fraction == 0)
    }

    @Test("a cleared goal encodes an explicit null target, not an absent key")
    func encodesAnExplicitNull() throws {
        let body = try JSONEncoder().encode(ReadingGoalUpdate(target: nil))
        let json = try #require(String(data: body, encoding: .utf8))
        #expect(json.contains("\"target\":null"))
    }
}

@Suite("Library composition")
struct LibraryCompositionTests {
    private func decode(_ json: String) throws -> LibraryComposition {
        try JSONDecoder().decode(LibraryComposition.self, from: Data(json.utf8))
    }

    private func dimension(_ pairs: [(String, Int64)], covered: Int64) -> CompositionDimension {
        let slices = pairs.map { CompositionSlice(label: $0.0, books: $0.1) }
        return CompositionDimension(
            slices: slices,
            coverage: MeasuredTotal(total: slices.reduce(0) { $0 + $1.books }, books: covered))
    }

    @Test("each dimension decodes with the coverage behind it")
    func decodesDimensionsAndCoverage() throws {
        let json = """
            {"books":1510,"ghosted_books":4,
             "formats":{"slices":[{"label":"EPUB","books":1400},{"label":"M4B","books":180}],
                        "coverage":{"total":1580,"books":1510}},
             "genres":{"slices":[{"label":"Fantasy","books":40}],
                       "coverage":{"total":40,"books":38}}}
            """
        let c = try decode(json)
        #expect(c.books == 1510)
        #expect(c.ghostedBooks == 4)
        #expect(c.formats.slices.first?.label == "EPUB")
        #expect(c.formats.coverage.books == 1510)
        // 1,580 placements over 1,510 books: seventy are held in two formats.
        #expect(c.formats.overlap == 70)
        #expect(c.genres.coverage.books == 38)
        // Nothing sent for these: an empty dimension, never a fake bar.
        #expect(c.publishers.isEmpty)
        #expect(!c.isEmpty)
    }

    @Test("a server that predates the composition still decodes to an empty one")
    func decodesWithoutDimensions() throws {
        let c = try decode(#"{"books":40}"#)
        #expect(c.books == 40)
        #expect(c.isEmpty)
    }

    @Test("the genre panel names its coverage rather than presenting a sample as whole")
    func genrePanelStatesCoverage() {
        var c = LibraryComposition()
        c.books = 1510
        c.formats = dimension([("EPUB", 1400)], covered: 1510)
        c.genres = dimension([("Fantasy", 40), ("Horror", 22)], covered: 58)

        let panels = StatsView.compositionPanels(c)

        #expect(panels.count == 5)
        let genres = panels.first { $0.title == "Genres" }
        #expect(genres?.note?.contains("hand-assigned") == true)
        #expect(genres?.note?.contains("58") == true)
        #expect(genres?.note?.contains("1,510") == true)
        // No publisher metadata anywhere: an empty state, not an empty chart.
        #expect(panels.first { $0.title == "Publishers" }?.dimension.isEmpty == true)
    }

    @Test("the format panel discloses books held in more than one format")
    func formatPanelDisclosesOverlap() {
        let single = CompositionDimension(
            slices: [CompositionSlice(label: "EPUB", books: 2)],
            coverage: MeasuredTotal(total: 2, books: 2))
        #expect(StatsView.overlapNote(single) == nil)

        let dual = CompositionDimension(
            slices: [CompositionSlice(label: "EPUB", books: 2)],
            coverage: MeasuredTotal(total: 3, books: 2))
        #expect(StatsView.overlapNote(dual) == "+1 book held in more than one format")
    }

    @Test("ghosted rows are named rather than left to make the bars not add up")
    func ghostedNote() {
        #expect(StatsView.ghostedNote(0) == nil)
        #expect(StatsView.ghostedNote(1)?.hasPrefix("1 book excluded") == true)
        #expect(StatsView.ghostedNote(4)?.hasPrefix("4 books excluded") == true)
    }
}
