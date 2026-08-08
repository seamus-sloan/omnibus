//  CheckInFlowTests.swift
//  The stage machine's pure logic: which success screen each write produces,
//  where the note field appears, and which covers are provider-hosted.

import Foundation
import Testing

@testable import omnibus

private func scanBook(uuid: String = "b-1", title: String = "Babel") -> ScanBook {
    ScanBook(uuid: uuid, title: title, authors: ["R. F. Kuang"], coverURL: nil, hasPhysical: false, isbn: nil)
}

private func externalMeta(cover: String? = "https://covers.example/x.jpg") -> ExternalBookMeta {
    ExternalBookMeta(
        isbn13: "9781250903440", title: "The Bee Sting", authors: ["Paul Murray"],
        year: "2023", pages: 656, publisher: nil, description: nil,
        coverURL: cover, source: "openlibrary"
    )
}

struct CheckInFlowTests {
    @Test func checkedInSuccessIsCelebratoryWithViewBook() {
        let success = CheckInFlow.checkedInSuccess(
            book: scanBook(), ref: BookRef(bookUUID: "b-1")
        )
        #expect(success.tone == .celebration)
        #expect(success.headline == "In your physical collection")
        #expect(success.bookUUID == "b-1")
        #expect(success.cover == .library(uuid: "b-1"))
    }

    @Test func checkedInSuccessPrefersTheCanonicalUUIDFromTheServer() {
        // A merged book's check-in answers with the primary's uuid — the
        // success screen must link and render through it.
        let success = CheckInFlow.checkedInSuccess(
            book: scanBook(uuid: "b-merged"), ref: BookRef(bookUUID: "b-primary")
        )
        #expect(success.bookUUID == "b-primary")
        #expect(success.cover == .library(uuid: "b-primary"))
    }

    @Test func addedSuccessUsesReturnedUUIDAndExternalCover() {
        let success = CheckInFlow.addedSuccess(
            meta: externalMeta(), ref: BookRef(bookUUID: "new-1")
        )
        #expect(success.tone == .celebration)
        #expect(success.bookUUID == "new-1")
        #expect(success.cover == .external(url: "https://covers.example/x.jpg"))
    }

    @Test func addedSuccessFallsBackToPlateWithoutACover() {
        let success = CheckInFlow.addedSuccess(
            meta: externalMeta(cover: nil), ref: BookRef(bookUUID: "new-1")
        )
        #expect(success.cover == .plate)
    }

    @Test func wishlistedSuccessIsQuietWithViewBook() {
        let success = CheckInFlow.wishlistedSuccess(
            meta: externalMeta(), ref: BookRef(bookUUID: "new-2")
        )
        #expect(success.tone == .quiet)
        #expect(success.headline == "On your wishlist")
        #expect(success.bookUUID == "new-2")
    }

    @Test func noteFieldShowsOnlyOnInLibraryUnowned() {
        #expect(CheckInFlow.showsNoteField(for: .inLibraryUnowned(book: scanBook())))
        #expect(!CheckInFlow.showsNoteField(for: .alreadyOwned(book: scanBook())))
        #expect(!CheckInFlow.showsNoteField(for: .onWishlist(book: scanBook())))
        #expect(!CheckInFlow.showsNoteField(for: .closeMatch(book: scanBook(), scanned: externalMeta())))
        #expect(!CheckInFlow.showsNoteField(for: .notInLibrary(online: externalMeta())))
        #expect(!CheckInFlow.showsNoteField(for: .unresolved))
    }

    @Test func resolveShouldClearSearchOnlyWhenLeavingTheScanStage() {
        // A resolve always fires from `.scan` — the ISBN field's stage — so
        // landing on any outcome (including `.unresolved`, whose fallback
        // reuses the scan page's title-search state) must clear the stale
        // query/results left over from it.
        #expect(CheckInFlow.resolveShouldClearSearch(from: .scan))
        #expect(!CheckInFlow.resolveShouldClearSearch(from: .outcome(.unresolved)))
        #expect(
            !CheckInFlow.resolveShouldClearSearch(
                from: .outcome(.inLibraryUnowned(book: scanBook()))
            )
        )
    }

    @Test func searchResponseShouldApplyOnlyWhenStageAndQueryAreUnchanged() {
        // A title search is a separate in-flight request — if a resolve lands
        // (stage moves on) or the field is retyped (query moves on) before it
        // returns, the late response must be dropped rather than repopulating
        // search state a resolve just cleared.
        #expect(
            CheckInFlow.searchResponseShouldApply(
                startedStage: .scan, currentStage: .scan,
                startedQuery: "babel", currentQuery: "babel"
            )
        )
        #expect(
            !CheckInFlow.searchResponseShouldApply(
                startedStage: .scan, currentStage: .outcome(.unresolved),
                startedQuery: "babel", currentQuery: "babel"
            )
        )
        #expect(
            !CheckInFlow.searchResponseShouldApply(
                startedStage: .scan, currentStage: .scan,
                startedQuery: "babel", currentQuery: "bee sting"
            )
        )
    }

    @Test func searchResponseAppliesWhenOnlySurroundingWhitespaceDiffers() {
        // The request is sent with the trimmed query while the field keeps the
        // raw text, and iOS appends a space to every accepted QuickType
        // suggestion — so this is the *typical* typed search, not an edge case.
        #expect(
            CheckInFlow.searchResponseShouldApply(
                startedStage: .scan, currentStage: .scan,
                startedQuery: "pride and prejudice", currentQuery: "pride and prejudice "
            )
        )
        #expect(
            CheckInFlow.searchResponseShouldApply(
                startedStage: .scan, currentStage: .scan,
                startedQuery: "babel", currentQuery: "  babel\n"
            )
        )
        // Trimming must not blunt the gate: real retyping still drops the
        // stale answer, whitespace or not.
        #expect(
            !CheckInFlow.searchResponseShouldApply(
                startedStage: .scan, currentStage: .scan,
                startedQuery: "babel", currentQuery: "babel two "
            )
        )
    }

    @Test func detailUUIDCoversAlreadyOwnedAndOnWishlistOnly() {
        #expect(CheckInFlow.detailUUID(for: .alreadyOwned(book: scanBook())) == "b-1")
        #expect(CheckInFlow.detailUUID(for: .onWishlist(book: scanBook())) == "b-1")
        #expect(CheckInFlow.detailUUID(for: .inLibraryUnowned(book: scanBook())) == nil)
        #expect(CheckInFlow.detailUUID(for: .notInLibrary(online: externalMeta())) == nil)
        #expect(CheckInFlow.detailUUID(for: .unresolved) == nil)
    }

    @Test func externalURLDetectionSplitsAbsoluteFromServerRelative() {
        #expect(CheckInFlow.isExternalURL("https://books.google.com/x.jpg"))
        #expect(CheckInFlow.isExternalURL("http://covers.openlibrary.org/x.jpg"))
        #expect(!CheckInFlow.isExternalURL("/covers/uuid/md"))
        #expect(!CheckInFlow.isExternalURL("covers/uuid/md"))
    }

    @Test func bookRefDecodesSnakeCaseBookUUID() throws {
        let ref = try JSONDecoder().decode(
            BookRef.self, from: Data(#"{"book_uuid":"b-9"}"#.utf8)
        )
        #expect(ref.bookUUID == "b-9")
    }

    @Test func detailLinesCarrySeriesThenTheJoinedFacts() {
        var meta = externalMeta()
        meta.series = "The Kingkiller Chronicle"
        meta.firstPublishYear = 2007
        meta.publisher = "DAW Books"
        meta.pages = 662
        #expect(
            CheckInFlow.detailLines(for: meta) == [
                "The Kingkiller Chronicle",
                "First published 2007 \u{b7} DAW Books \u{b7} 662 pages",
            ]
        )
    }

    @Test func detailLinesAreEmptyWhenTheProviderCarriedNothing() {
        var meta = externalMeta()
        meta.publisher = "   "
        meta.pages = 0
        // externalMeta() has no series or first-publish year; a blank
        // publisher and a zero page count don't count as facts either.
        #expect(CheckInFlow.detailLines(for: meta).isEmpty)
    }
}
