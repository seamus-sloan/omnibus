//  ComicPositionTests.swift
//  The comic page ↔ progress record mapping — the arithmetic every device
//  agrees on.
//
//  These mirror the web pager's tests for the same math: the anchor
//  round-trip, the percent's endpoint behaviour, and the resume fallback
//  chain. A divergence here is a book that resumes on a different page than
//  the one the other client saved.

import Foundation
import Testing

@testable import omnibus

@Suite("Comic page anchor")
struct ComicAnchorTests {
    @Test("an anchor round-trips through its parser")
    func anchorRoundTrips() {
        #expect(ComicPosition.anchor(page: 0) == "comic-page:0")
        #expect(ComicPosition.parseAnchor(ComicPosition.anchor(page: 12)) == 12)
    }

    @Test("a real EPUB CFI is not misread as a page")
    func foreignPositionsAreRejected() {
        // The prefix is what makes the anchor self-describing — a CFI or
        // garbage must fall through to the percent fallback, never parse.
        #expect(ComicPosition.parseAnchor("epubcfi(/6/4!/4/2)") == nil)
        #expect(ComicPosition.parseAnchor("comic-page:") == nil)
        #expect(ComicPosition.parseAnchor("comic-page:12b") == nil)
        #expect(ComicPosition.parseAnchor("") == nil)
    }
}

@Suite("Comic percent")
struct ComicPercentTests {
    @Test("first and last pages map to the ends of the bar")
    func endpointsMapToEnds() {
        // Same cases the web pager pins: page 0 of a long book reads as 0,
        // the last page as 100, a single-page book as 100, and an empty
        // archive degrades to 0 rather than dividing by zero.
        #expect(ComicPosition.percent(page: 0, count: 219) == 0)
        #expect(ComicPosition.percent(page: 218, count: 219) == 100)
        #expect(ComicPosition.percent(page: 0, count: 1) == 100)
        #expect(ComicPosition.percent(page: 0, count: 0) == 0)
    }

    @Test("the midpoint rounds like the web pager")
    func midpointMatchesTheWebMath() {
        // (10+1)*100/20 = 55 — the +1 is what makes the last page 100.
        #expect(ComicPosition.percent(page: 10, count: 20) == 55)
    }
}

@Suite("Comic resume page")
struct ComicStartPageTests {
    @Test("the exact anchor wins over the percent")
    func anchorWins() {
        let page = ComicPosition.startPage(
            anchor: ComicPosition.anchor(page: 7), percent: 100, count: 20
        )
        #expect(page == 7)
    }

    @Test("an anchor past the end of a shrunken re-scan is clamped")
    func anchorIsClamped() {
        let page = ComicPosition.startPage(
            anchor: ComicPosition.anchor(page: 50), percent: nil, count: 20
        )
        #expect(page == 19)
    }

    @Test("the percent inverts when there is no anchor")
    func percentFallsBack() {
        // A CFI is not an anchor, so a foreign position falls through to
        // the percent — round(50/100 × 10) − 1 = 4.
        #expect(
            ComicPosition.startPage(anchor: "epubcfi(/6/4)", percent: 50, count: 10) == 4
        )
        #expect(ComicPosition.startPage(anchor: nil, percent: 0, count: 10) == 0)
        #expect(ComicPosition.startPage(anchor: nil, percent: 100, count: 10) == 9)
    }

    @Test("no position at all opens on page one")
    func bareOpenStartsAtZero() {
        #expect(ComicPosition.startPage(anchor: nil, percent: nil, count: 10) == 0)
        #expect(ComicPosition.startPage(anchor: nil, percent: nil, count: 0) == 0)
    }
}

@Suite("Resume card fraction")
struct ComicResumeFractionTests {
    private func record(percent: Int64?) -> ProgressRecord {
        ProgressRecord(
            bookUUID: "u1", format: .epub, epubCFI: ComicPosition.anchor(page: 3),
            audioPositionSeconds: nil, progressPercent: percent,
            updatedAt: 100, clientUpdatedAt: 100, bookFileID: nil
        )
    }

    private func point(percent: Int64?) -> ResumePoint {
        ResumePoint(
            record: record(percent: percent),
            book: Book(id: 1, filename: "c.cbz"),
            totalDurationSeconds: nil, chapterNumber: nil, chapterCount: nil
        )
    }

    @Test("a comic record's percent drives the card's bar")
    func percentBecomesTheFraction() {
        #expect(point(percent: 40).fraction == 0.4)
        #expect(point(percent: 100).fraction == 1.0)
    }

    @Test("a CFI-only reading record still has no honest fraction")
    func cfiOnlyRecordsStayBarless() {
        #expect(point(percent: nil).fraction == nil)
    }
}
