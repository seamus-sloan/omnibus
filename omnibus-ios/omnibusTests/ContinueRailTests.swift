//  ContinueRailTests.swift
//  What the Continue rail considers one card, and how a saved position is
//  spliced into it offline.
//
//  Both were keyed on the book uuid alone, which is only correct until someone
//  reads and listens to the same book — then the two positions were one card
//  that overwrote itself, and the carousel rendered a duplicate identity.

import Foundation
import Testing

@testable import omnibus

private func book(
    id: Int64 = 1, uuid: String = "book-1", title: String = "Immersive Voyage",
    formats: [String] = ["epub", "m4b"]
) -> Book {
    Book(id: id, filename: "\(uuid).epub", title: title, uniqueIdentifier: uuid, formats: formats)
}

private func record(
    uuid: String = "book-1", format: ProgressFormat = .epub, at seconds: Double? = nil,
    cfi: String? = nil, clock: Int64 = 1_700_000_000
) -> ProgressRecord {
    ProgressRecord(
        bookUUID: uuid, format: format, epubCFI: cfi, audioPositionSeconds: seconds,
        updatedAt: clock, clientUpdatedAt: clock
    )
}

private func point(
    _ record: ProgressRecord, book: Book = book(), duration: Double? = nil,
    chapter: Int64? = nil, chapters: Int64? = nil
) -> ResumePoint {
    ResumePoint(
        record: record, book: book, totalDurationSeconds: duration,
        chapterNumber: chapter, chapterCount: chapters
    )
}

// MARK: - Card identity

@Suite("Resume card identity")
struct ResumePointIdentityTests {
    @Test("reading and listening to one book are two cards, not one")
    func formatsAreDistinctCards() {
        // The carousel renders these in a `ForEach` keyed on `id`. Sharing one
        // gave SwiftUI a duplicate identity: state moved between the two cards
        // and the page count stopped agreeing with the dots below it.
        let reading = point(record(format: .epub, cfi: "epubcfi(/6/4)"))
        let listening = point(record(format: .audio, at: 900))
        #expect(reading.id != listening.id)
    }

    @Test("the same position is the same card across a refresh")
    func identityIsStableForOneFormat() {
        // Stability is what lets the carousel stay on the card you're looking at
        // when the rail is rewritten by a save, instead of snapping to the front.
        let first = point(record(format: .audio, at: 900))
        let later = point(record(format: .audio, at: 1_500))
        #expect(first.id == later.id)
    }

    @Test("a card matches only its own book and format")
    func matchesPairsOnBookAndFormat() {
        let reading = point(record(format: .epub))
        #expect(reading.matches(record(format: .epub)))
        #expect(!reading.matches(record(format: .audio)))
        #expect(!reading.matches(record(uuid: "book-2", format: .epub)))
    }
}

// MARK: - Progress fraction

@Suite("Resume card fraction")
struct ResumePointFractionTests {
    @Test("a listening position is a fraction of the whole duration")
    func audioFractionIsProportional() {
        let listening = point(record(format: .audio, at: 900), duration: 3_600)
        #expect(listening.fraction == 0.25)
    }

    @Test("a reading position has no fraction even with audio fields set")
    func epubHasNoFraction() {
        // An EPUB position is a CFI; there is no percentage to derive from one.
        // The fields are only ever populated for audio, but a card that read
        // them without checking the format would draw the wrong book's bar.
        let reading = point(record(format: .epub, at: 900), duration: 3_600)
        #expect(reading.fraction == nil)
    }

    @Test("a listening position with no known duration has no fraction")
    func audioWithoutDurationHasNoFraction() {
        #expect(point(record(format: .audio, at: 900)).fraction == nil)
    }

    @Test("a position past the end still reports a full bar, not an overflowing one")
    func fractionIsClamped() {
        let overrun = point(record(format: .audio, at: 4_000), duration: 3_600)
        #expect(overrun.fraction == 1)
    }
}

// MARK: - Offline splice

@Suite("Continue rail splice")
struct ResumeSpliceTests {
    @Test("a listening position leaves the reading card alone")
    func listeningDoesNotOverwriteReading() throws {
        // The bug this pins: one card per book meant starting the audiobook
        // rewrote the reading card in place — same book, wrong verb, and the
        // reading position it was holding was gone.
        let rail = [point(record(format: .epub, cfi: "epubcfi(/6/4)"))]
        let cards = try #require(
            UserDataService.splice(
                record(format: .audio, at: 900, clock: 1_700_000_100), book: book(), into: rail
            )
        )

        #expect(cards.count == 2)
        #expect(cards.first?.record.format == .audio)
        #expect(cards.last?.record.format == .epub)
        #expect(cards.last?.record.epubCFI == "epubcfi(/6/4)")
    }

    @Test("saving a position moves its own card to the front")
    func savedCardMovesToTheFront() {
        let rail = [
            point(record(uuid: "book-2"), book: book(id: 2, uuid: "book-2")),
            point(record(uuid: "book-1")),
        ]
        let spliced = UserDataService.splice(
            record(uuid: "book-1", cfi: "epubcfi(/6/9)"), book: nil, into: rail
        )

        #expect(spliced?.map(\.id) == ["book-1:epub", "book-2:epub"])
        #expect(spliced?.first?.record.epubCFI == "epubcfi(/6/9)")
    }

    @Test("updating a card keeps the totals only the server can compute")
    func serverComputedTotalsSurviveAnUpdate() {
        // This device can't derive a duration or a chapter index. Dropping them
        // on each save would blank the card's bar between server reads.
        let rail = [
            point(record(format: .audio, at: 900), duration: 3_600, chapter: 2, chapters: 12)
        ]
        let spliced = UserDataService.splice(
            record(format: .audio, at: 1_800), book: nil, into: rail
        )

        #expect(spliced?.first?.totalDurationSeconds == 3_600)
        #expect(spliced?.first?.chapterCount == 12)
        #expect(spliced?.first?.fraction == 0.5)
    }

    @Test("a book that can't be resolved leaves the rail untouched")
    func unresolvableBookIsNotAdded() {
        // Better no card than one with no title, author, or cover to draw.
        #expect(UserDataService.splice(record(uuid: "ghost"), book: nil, into: []) == nil)
    }

    @Test("the rail stays within the length the carousel pages through")
    func railIsCappedAtTheCarouselLength() {
        let rail = (1...5).map { index in
            point(
                record(uuid: "book-\(index)"),
                book: book(id: Int64(index), uuid: "book-\(index)")
            )
        }
        let spliced = UserDataService.splice(
            record(uuid: "book-9"), book: book(id: 9, uuid: "book-9"), into: rail
        )

        #expect(spliced?.count == 5)
        #expect(spliced?.first?.id == "book-9:epub")
        // The oldest card is the one that falls off the end.
        #expect(spliced?.contains { $0.id == "book-5:epub" } == false)
    }
}

// MARK: - Wire decode

@Suite("Resume point playback rate")
struct ResumePointPlaybackRateTests {
    @Test("carries the saved rate under the wire key the server writes")
    func rateRoundTripsUnderTheWireKey() throws {
        var p = point(record(format: .audio, at: 450), duration: 1_200)
        p.playbackRate = 2.0
        let data = try JSONEncoder().encode(p)
        // Pin the snake_case wire name, not just a same-struct round-trip.
        let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        #expect(object?["playback_rate"] as? Double == 2.0)
        #expect(try JSONDecoder().decode(ResumePoint.self, from: data).playbackRate == 2.0)
    }

    @Test("a payload without a saved rate decodes to nil, read as 1x")
    func missingRateDecodesToNil() throws {
        let data = try JSONEncoder().encode(point(record(format: .audio, at: 450), duration: 1_200))
        let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        #expect(object?.keys.contains("playback_rate") == false)
        #expect(try JSONDecoder().decode(ResumePoint.self, from: data).playbackRate == nil)
    }
}
