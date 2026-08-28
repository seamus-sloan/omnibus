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
