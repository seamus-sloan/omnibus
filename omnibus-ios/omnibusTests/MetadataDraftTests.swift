//  MetadataDraftTests.swift
//  The metadata editor's pure logic: which fields a save actually sends,
//  and what a typed entry commits as a chip.

import Foundation
import Testing

@testable import omnibus

private func gatsby() -> Book {
    var book = Book(id: 7, filename: "gatsby.epub")
    book.title = "The Great Gatsby"
    book.creators = [Contributor(name: "F. Scott Fitzgerald")]
    book.subjects = ["Classics", "Novel"]
    book.genres = ["Literary Fiction"]
    book.series = "—"
    book.publisher = "Scribner"
    book.language = "en"
    book.isbn13 = "9780743273565"
    book.isbn10 = "0743273567"
    book.printPages = 180
    return book
}

struct MetadataDraftTests {
    @Test func draftFromBookCarriesSubjectsAsTags() {
        let draft = MetadataDraft(book: gatsby())
        #expect(draft.tags == ["Classics", "Novel"])
        #expect(draft.authors == ["F. Scott Fitzgerald"])
        #expect(draft.title == "The Great Gatsby")
    }

    @Test func payloadSendsOnlyTheFieldsThatChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.title = "Trimalchio"
        draft.tags = ["Classics", "Novel", "Jazz Age"]

        let payload = draft.payload(since: loaded)
        #expect(payload.title == "Trimalchio")
        #expect(payload.subjects == ["Classics", "Novel", "Jazz Age"])
        // Untouched fields stay absent so the merge endpoint leaves them
        // alone — sending them would pin scanned values against a rescan.
        #expect(payload.creators == nil)
        #expect(payload.series == nil)
        #expect(payload.publisher == nil)
        #expect(payload.language == nil)
        #expect(payload.description == nil)
    }

    @Test func draftFromBookCarriesGenresSeparatelyFromTags() {
        let draft = MetadataDraft(book: gatsby())
        #expect(draft.genres == ["Literary Fiction"])
        #expect(draft.tags == ["Classics", "Novel"])
    }

    @Test func payloadSendsGenresOnlyWhenTheListChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.genres = ["Literary Fiction", "Tragedy"]

        let payload = draft.payload(since: loaded)
        #expect(payload.genres == ["Literary Fiction", "Tragedy"])
        // Editing genres must not drag the tag list along with it — they are
        // two independent wholesale-replace overrides.
        #expect(payload.subjects == nil)
    }

    @Test func payloadOmitsGenresWhenOnlyTagsChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.tags = ["Classics"]

        let payload = draft.payload(since: loaded)
        #expect(payload.genres == nil)
        #expect(payload.subjects == ["Classics"])
    }

    @Test func payloadOmitsTagsWhenTheListIsUntouched() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.publisher = "Modern Library"

        let payload = draft.payload(since: loaded)
        #expect(payload.subjects == nil)
        #expect(payload.publisher == "Modern Library")
    }

    @Test func payloadSendsTheWholeTagListWhenOneTagIsRemoved() {
        // The server's subjects override replaces wholesale, so a removal is
        // expressed as the surviving list — not a delta.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.tags = ["Classics"]

        #expect(draft.payload(since: loaded).subjects == ["Classics"])
    }

    @Test func payloadClearsAScalarFieldWithAnEmptyString() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.publisher = ""

        #expect(draft.payload(since: loaded).publisher == "")
    }

    @Test func payloadReplacesTheWholeCreatorListWhenAuthorsChange() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.authors = ["F. Scott Fitzgerald", "Zelda Fitzgerald"]

        #expect(draft.payload(since: loaded).creators == [
            MetadataOverridesPayload.Creator(name: "F. Scott Fitzgerald"),
            MetadataOverridesPayload.Creator(name: "Zelda Fitzgerald"),
        ])
    }

    // MARK: - ISBN-10 and print pages

    @Test func draftFromBookCarriesIsbn10AndPrintPages() {
        let draft = MetadataDraft(book: gatsby())
        #expect(draft.isbn10 == "0743273567")
        // Held as text, so a book with a value arrives as its digits.
        #expect(draft.printPages == "180")
    }

    @Test func draftFromABookWithoutTheNewFieldsLeavesThemBlank() {
        // A server that predates the fields omits them; both must read as
        // "nothing set" rather than decoding to a placeholder.
        var book = gatsby()
        book.isbn10 = nil
        book.printPages = nil

        let draft = MetadataDraft(book: book)
        #expect(draft.isbn10 == "")
        #expect(draft.printPages == "")
    }

    @Test func payloadSendsIsbn10WhenChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.isbn10 = "0141182636"

        #expect(draft.payload(since: loaded).isbn10 == "0141182636")
    }

    @Test func payloadClearsAPopulatedIsbn10WithAnEmptyString() {
        // Same empty-string-clears convention as ISBN-13 — the one scalar
        // sentinel the server honours.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.isbn10 = ""

        #expect(draft.payload(since: loaded).isbn10 == "")
    }

    @Test func payloadSendsPrintPagesWhenChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "412"

        #expect(draft.payload(since: loaded).print_pages == 412)
    }

    @Test func payloadOmitsPrintPagesWhenTheFieldIsBlanked() {
        // There is no "empty" integer on the wire, so a blanked field means
        // leave the override alone — not clear it. The web editor's
        // `build_overrides` makes the same choice; the two must agree.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = ""

        #expect(draft.payload(since: loaded).print_pages == nil)
    }

    @Test func payloadOmitsPrintPagesWhenRetypedUnchanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = " 180 "

        #expect(draft.payload(since: loaded).print_pages == nil)
    }

    // MARK: - The nil baseline, which is what a real library has

    // Both fields are override-only — `books/projection.rs` sets them to
    // `None` with "No scanned baseline" — so an unedited book has nil for
    // both and *every first edit* takes this path. `gatsby()` seeds values,
    // so without these the load-bearing `parsed == loaded.printPagesValue`
    // comparison against nil is never exercised.

    private static func unedited() -> Book {
        var book = gatsby()
        book.isbn10 = nil
        book.printPages = nil
        return book
    }

    @Test func payloadSendsPrintPagesOnABookThatNeverHadOne() {
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.printPages = "412"

        #expect(draft.payload(since: loaded).print_pages == 412)
    }

    @Test func payloadSendsIsbn10OnABookThatNeverHadOne() {
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "0141182636"

        #expect(draft.payload(since: loaded).isbn10 == "0141182636")
    }

    @Test func payloadOmitsBothOnAnUneditedBookWhenNeitherWasTouched() {
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.title = "Trimalchio"

        let payload = draft.payload(since: loaded)
        #expect(payload.print_pages == nil)
        // Not `""`: the field was blank and stayed blank, so there is no
        // clear to express and the key must not appear at all.
        #expect(payload.isbn10 == nil)
    }

    // MARK: - Whitespace

    @Test func payloadSendsIsbn10Trimmed() {
        // The field is `.asciiCapable`, so unlike the `.numberPad` ISBN-13
        // beside it a pasted value can carry whitespace — which the server
        // rejects rather than strips, taking every other edit in the same
        // body down with it.
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "  0141182636\n"

        #expect(draft.payload(since: loaded).isbn10 == "0141182636")
    }

    @Test func payloadOmitsIsbn10WhenOnlyItsWhitespaceChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.isbn10 = " 0743273567 "

        #expect(draft.payload(since: loaded).isbn10 == nil)
    }

    // MARK: - An empty body is not a no-op server-side

    @Test func payloadIsEmptyWhenAReformattedEntryNormalizesBackToLoaded() {
        // The editor's dirty check compares raw text, so Save enables here.
        // If this body were posted, `merge_one_in_tx` would insert an empty
        // override row and `apply_overrides` would report has_override =
        // true forever — the book reads "Edited" with nothing to revert.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = " 180 "

        #expect(draft != loaded, "precondition: the editor would enable Save")
        #expect(draft.payload(since: loaded).isEmpty)
    }

    @Test func payloadIsEmptyWhenPrintPagesIsRetypedWithALeadingZero() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "0180"

        #expect(draft != loaded)
        #expect(draft.payload(since: loaded).isEmpty)
    }

    @Test func payloadIsNotEmptyWhenSomethingActuallyChanged() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.title = "Trimalchio"

        #expect(!draft.payload(since: loaded).isEmpty)
    }

    // MARK: - validate, which runs before a save touches any state

    @Test func validatePassesForAnOrdinaryEdit() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "412"
        draft.isbn10 = "0141182636"

        try draft.validate(since: loaded)
    }

    @Test func validateRejectsANonNumericPrintPagesEntry() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "about 400"

        #expect(throws: MetadataDraftError.invalidPrintPages) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateRejectsAPrintPagesEntryBelowTheServersFloor() {
        // "0" is one tap away on a number pad, and the server's bound is
        // `1..=PRINT_PAGES_MAX`. Catching it here makes it read as the range
        // error it is rather than the "not a number" one it isn't.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "0"

        #expect(throws: MetadataDraftError.printPagesOutOfRange) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateRejectsAPrintPagesEntryAboveTheServersCeiling() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = String(MetadataDraft.printPagesMax + 1)

        #expect(throws: MetadataDraftError.printPagesOutOfRange) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateRejectsBlankingAPrintPagesValueThatWasSet() {
        // The wire field is an integer with no empty-string sentinel, so this
        // save would drop the edit silently. Refusing it is the difference
        // between "can't do that" and a Save that lies.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = ""

        #expect(throws: MetadataDraftError.printPagesNotClearable) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateAllowsABlankPrintPagesWhenTheBookNeverHadOne() throws {
        // Nothing to clear, so nothing to refuse — otherwise every book in an
        // unedited library would be unsaveable.
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.title = "Trimalchio"

        try draft.validate(since: loaded)
    }

    @Test func validateRejectsAHyphenatedIsbn10() {
        // The form printed on most copyright pages. The server strips
        // nothing, so without this the whole save 400s.
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "0-7432-7356-7"

        #expect(throws: MetadataDraftError.invalidIsbn10) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateRejectsAnIsbn10OfTheWrongLength() {
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "9780743273565"

        #expect(throws: MetadataDraftError.invalidIsbn10) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateAcceptsAnIsbn10WithAnXCheckDigit() throws {
        // Why the field is `.asciiCapable` rather than `.numberPad`.
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "043942089X"

        try draft.validate(since: loaded)
    }

    @Test func validateRejectsAnXAnywhereButTheCheckDigit() {
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "X439420891"

        #expect(throws: MetadataDraftError.invalidIsbn10) {
            try draft.validate(since: loaded)
        }
    }

    @Test func validateAcceptsAnIsbn10ClearedToBlank() throws {
        // Empty *is* the clear sentinel for this one, unlike print pages.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.isbn10 = ""

        try draft.validate(since: loaded)
    }

    @Test func validateAcceptsAnIsbn10WithSurroundingWhitespace() throws {
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = " 0141182636 "

        try draft.validate(since: loaded)
    }

    @Test func validateRejectsNonASCIIDigitsInAnIsbn10() {
        // `Character.isNumber` is true for these; the server's check is
        // `is_ascii_digit`, so anything accepted here must pass there.
        let loaded = MetadataDraft(book: Self.unedited())
        var draft = loaded
        draft.isbn10 = "٠١٤١١٨٢٦٣٦"

        #expect(throws: MetadataDraftError.invalidIsbn10) {
            try draft.validate(since: loaded)
        }
    }

    @Test func payloadOmitsTheNewFieldsWhenNothingElseChangedThem() {
        // AC3, and the merge guarantee this ticket exists to protect: an
        // edit elsewhere must not restate ISBN-10 or the print page count,
        // because `POST /overrides` merges — a client that restates a stale
        // value clobbers whatever the other editor set.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.title = "Trimalchio"

        let payload = draft.payload(since: loaded)
        #expect(payload.isbn10 == nil)
        #expect(payload.print_pages == nil)
        #expect(payload.genres == nil)
    }

    @Test func encodedPayloadOmitsUntouchedNewFieldsEntirely() throws {
        // The merge is keyed on the JSON key being *absent*, not null — so
        // assert the encoded body, which is what the server actually reads.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.title = "Trimalchio"

        let payload = draft.payload(since: loaded)
        let data = try JSONEncoder().encode(payload)
        let json = try #require(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        #expect(json["title"] as? String == "Trimalchio")
        #expect(!json.keys.contains("isbn10"))
        #expect(!json.keys.contains("print_pages"))
        #expect(!json.keys.contains("genres"))
    }

    @Test func encodedPayloadUsesTheServersKeyNamesForChangedFields() throws {
        // The positive half, and the one that actually pins the outbound
        // names. `MetadataOverrides` has no `deny_unknown_fields`, so a
        // misspelled key deserializes to nothing: the server returns 200 with
        // the full book and the editor dismisses on success while the value
        // is silently discarded. Asserting only *absence* would not catch it,
        // and neither would reading the Swift properties back.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "412"
        draft.isbn10 = "0141182636"
        draft.genres = ["Literary Fiction", "Tragedy"]

        let data = try JSONEncoder().encode(draft.payload(since: loaded))
        let json = try #require(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        #expect(json["print_pages"] as? Int == 412)
        #expect(json["isbn10"] as? String == "0141182636")
        #expect(json["genres"] as? [String] == ["Literary Fiction", "Tragedy"])
        // Not camelCase — the sibling `Book` decoder uses `printPages` for
        // this same wire key, which is exactly the rename that would slip by.
        #expect(!json.keys.contains("printPages"))
    }
}

/// The wire half of the same contract: the server's key names, which no
/// amount of draft-level testing would catch if they were wrong.
struct BookOverrideFieldDecodeTests {
    @Test func bookDecodesTheOverrideOnlyFields() throws {
        let json = """
            {"id":7,"filename":"gatsby.epub","title":"The Great Gatsby",
             "genres":["Literary Fiction"],"isbn10":"0743273567",
             "print_pages":180,"page_count":24}
            """
        let book = try JSONDecoder().decode(Book.self, from: Data(json.utf8))
        #expect(book.isbn10 == "0743273567")
        #expect(book.printPages == 180)
        #expect(book.genres == ["Literary Fiction"])
        // `print_pages` is the print edition's count; `page_count` is the CBZ
        // archive's image count. Decoding one into the other would put a
        // comic's image count on the metadata editor.
        #expect(book.pageCount == 24)
    }

    @Test func bookDecodesWithoutTheOverrideOnlyFields() throws {
        // The server omits each of them when unset, and a list projection
        // omits them always — absence must read as "nothing set".
        let json = """
            {"id":7,"filename":"gatsby.epub","title":"The Great Gatsby"}
            """
        let book = try JSONDecoder().decode(Book.self, from: Data(json.utf8))
        #expect(book.isbn10 == nil)
        #expect(book.printPages == nil)
        #expect(book.genres.isEmpty)
    }
}

struct ChipEntryTests {
    @Test func committedTrimsSurroundingWhitespace() {
        let chip = ChipEntry.committed(from: "  Jazz Age  ", existing: [], deduplicating: true)
        #expect(chip == "Jazz Age")
    }

    @Test func committedRefusesABlankEntry() {
        #expect(ChipEntry.committed(from: "   ", existing: [], deduplicating: false) == nil)
        #expect(ChipEntry.committed(from: "", existing: [], deduplicating: true) == nil)
        // Pasted values commonly carry a trailing newline; newline-only input
        // is still blank.
        #expect(ChipEntry.committed(from: "\n", existing: [], deduplicating: true) == nil)
        #expect(ChipEntry.committed(from: " \n ", existing: [], deduplicating: false) == nil)
    }

    @Test func committedTrimsATrailingNewlineFromAPastedValue() {
        let chip = ChipEntry.committed(from: "Jazz Age\n", existing: [], deduplicating: true)
        #expect(chip == "Jazz Age")
    }

    @Test func committedRefusesADuplicateTagIgnoringCase() {
        let chip = ChipEntry.committed(
            from: "classics", existing: ["Classics", "Novel"], deduplicating: true
        )
        #expect(chip == nil)
    }

    @Test func committedAllowsADuplicateAuthorName() {
        // Two credited contributors can legitimately share a name, so the
        // authors field does not deduplicate.
        let chip = ChipEntry.committed(
            from: "F. Scott Fitzgerald",
            existing: ["F. Scott Fitzgerald"],
            deduplicating: false
        )
        #expect(chip == "F. Scott Fitzgerald")
    }
}
