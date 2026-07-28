//  ScanCodecTests.swift
//  The scan flow's wire contract with `omnibus_shared`'s scan types.
//
//  These models are hand-mirrored from Rust, so drift is silent: the wishlist
//  body shipped without the `source` the server requires and with a `note` it
//  has no field for, which `Json<WishlistAddRequest>` rejected as a 422 before
//  the handler ran — the button did nothing. Decoding drifted the same way,
//  with `on_wishlist` falling through to `.unresolved` ("couldn't identify that
//  book") for a book sitting on the caller's own wishlist.

import Foundation
import Testing

@testable import omnibus

private func meta(isbn: String = "9780000000002") -> ExternalBookMeta {
    ExternalBookMeta(
        isbn13: isbn, title: "Verify Book", authors: ["A Author"], year: nil, pages: nil,
        publisher: nil, description: nil, coverURL: nil, source: "open_library"
    )
}

private func encodedObject(_ value: some Encodable) throws -> [String: Any] {
    let data = try JSONEncoder().encode(value)
    return try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
}

// MARK: - Request encoding

@Suite("Wishlist request encoding")
struct WishlistAddRequestCodecTests {
    @Test("carries the source the server has no default for")
    func encodesSource() throws {
        let object = try encodedObject(
            WishlistAddRequest(bookUUID: nil, meta: meta(), source: .scan)
        )
        #expect(object["source"] as? String == "scan")
    }

    @Test("names an existing book with a snake_case book_uuid")
    func encodesBookUUIDKey() throws {
        let object = try encodedObject(
            WishlistAddRequest(bookUUID: "book-1", meta: nil, source: .detail)
        )
        #expect(object["book_uuid"] as? String == "book-1")
        #expect(object["source"] as? String == "detail")
    }

    @Test("sends no note — a wishlist entry has nowhere to put one")
    func omitsNote() throws {
        let object = try encodedObject(
            WishlistAddRequest(bookUUID: nil, meta: meta(), source: .scan)
        )
        #expect(object["note"] == nil)
    }

    @Test("round-trips the provider tag inside meta verbatim")
    func preservesMetaSource() throws {
        let object = try encodedObject(
            WishlistAddRequest(bookUUID: nil, meta: meta(), source: .scan)
        )
        let nested = try #require(object["meta"] as? [String: Any])
        #expect(nested["source"] as? String == "open_library")
        #expect(nested["isbn13"] as? String == "9780000000002")
    }
}

// MARK: - Outcome decoding

@Suite("Scan outcome decoding")
struct ScanOutcomeCodecTests {
    private func decode(_ json: String) throws -> ScanOutcome {
        try JSONDecoder().decode(ScanOutcome.self, from: Data(json.utf8))
    }

    @Test("a book already on the wishlist is its own outcome, not unresolved")
    func decodesOnWishlist() throws {
        let outcome = try decode(
            """
            {"kind":"on_wishlist","book":{"uuid":"book-1","title":"Verify Book",
             "authors":["A Author"],"cover_url":null,"has_physical":false}}
            """
        )
        guard case let .onWishlist(book) = outcome else {
            Issue.record("expected .onWishlist, got \(outcome)")
            return
        }
        #expect(book.uuid == "book-1")
    }

    /// This list is hand-maintained and cannot notice a variant added to
    /// `ScanOutcome` in `shared/src/scan.rs` — a new kind decodes as
    /// `.unresolved` and this still passes. Extend it in the same change that
    /// adds one; that obligation is the whole reason the case below exists.
    @Test("each kind known to this build maps to its own case, not the fallback")
    func decodesEachKnownKind() throws {
        let book = """
            {"uuid":"book-1","title":"Verify Book","authors":["A Author"],
             "cover_url":null,"has_physical":false}
            """
        let online = """
            {"isbn13":"9780000000002","title":"Verify Book","authors":["A Author"],
             "year":null,"pages":null,"publisher":null,"description":null,
             "cover_url":null,"source":"open_library"}
            """
        #expect(isAlreadyOwned(try decode(#"{"kind":"already_owned","book":\#(book)}"#)))
        #expect(isOnWishlist(try decode(#"{"kind":"on_wishlist","book":\#(book)}"#)))
        #expect(isInLibraryUnowned(try decode(#"{"kind":"in_library_unowned","book":\#(book)}"#)))
        #expect(
            isCloseMatch(
                try decode(#"{"kind":"close_match","book":\#(book),"scanned":\#(online)}"#)
            )
        )
        #expect(isNotInLibrary(try decode(#"{"kind":"not_in_library","online":\#(online)}"#)))
        #expect(isUnresolved(try decode(#"{"kind":"unresolved"}"#)))
    }

    /// A kind this build predates must read as "couldn't identify it" rather
    /// than throwing — the fallback is deliberate, and the point of the test
    /// above is that no *current* kind reaches it.
    @Test("an unrecognized kind falls back to unresolved")
    func decodesUnknownKindAsUnresolved() throws {
        #expect(isUnresolved(try decode(#"{"kind":"invented_later"}"#)))
    }

    private func isAlreadyOwned(_ o: ScanOutcome) -> Bool {
        if case .alreadyOwned = o { return true }
        return false
    }
    private func isOnWishlist(_ o: ScanOutcome) -> Bool {
        if case .onWishlist = o { return true }
        return false
    }
    private func isInLibraryUnowned(_ o: ScanOutcome) -> Bool {
        if case .inLibraryUnowned = o { return true }
        return false
    }
    private func isCloseMatch(_ o: ScanOutcome) -> Bool {
        if case .closeMatch = o { return true }
        return false
    }
    private func isNotInLibrary(_ o: ScanOutcome) -> Bool {
        if case .notInLibrary = o { return true }
        return false
    }
    private func isUnresolved(_ o: ScanOutcome) -> Bool {
        if case .unresolved = o { return true }
        return false
    }
}
