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

    @Test func payloadSendsOnlyTheFieldsThatChanged() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.title = "Trimalchio"
        draft.tags = ["Classics", "Novel", "Jazz Age"]

        let payload = try draft.payload(since: loaded)
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

    @Test func payloadSendsGenresOnlyWhenTheListChanged() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.genres = ["Literary Fiction", "Tragedy"]

        let payload = try draft.payload(since: loaded)
        #expect(payload.genres == ["Literary Fiction", "Tragedy"])
        // Editing genres must not drag the tag list along with it — they are
        // two independent wholesale-replace overrides.
        #expect(payload.subjects == nil)
    }

    @Test func payloadOmitsGenresWhenOnlyTagsChanged() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.tags = ["Classics"]

        let payload = try draft.payload(since: loaded)
        #expect(payload.genres == nil)
        #expect(payload.subjects == ["Classics"])
    }

    @Test func payloadOmitsTagsWhenTheListIsUntouched() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.publisher = "Modern Library"

        let payload = try draft.payload(since: loaded)
        #expect(payload.subjects == nil)
        #expect(payload.publisher == "Modern Library")
    }

    @Test func payloadSendsTheWholeTagListWhenOneTagIsRemoved() throws {
        // The server's subjects override replaces wholesale, so a removal is
        // expressed as the surviving list — not a delta.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.tags = ["Classics"]

        #expect(try draft.payload(since: loaded).subjects == ["Classics"])
    }

    @Test func payloadClearsAScalarFieldWithAnEmptyString() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.publisher = ""

        #expect(try draft.payload(since: loaded).publisher == "")
    }

    @Test func payloadReplacesTheWholeCreatorListWhenAuthorsChange() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.authors = ["F. Scott Fitzgerald", "Zelda Fitzgerald"]

        #expect(try draft.payload(since: loaded).creators == [
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

    @Test func payloadSendsIsbn10WhenChanged() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.isbn10 = "0141182636"

        #expect(try draft.payload(since: loaded).isbn10 == "0141182636")
    }

    @Test func payloadClearsAPopulatedIsbn10WithAnEmptyString() throws {
        // Same empty-string-clears convention as ISBN-13 — the one scalar
        // sentinel the server honours.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.isbn10 = ""

        #expect(try draft.payload(since: loaded).isbn10 == "")
    }

    @Test func payloadSendsPrintPagesWhenChanged() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "412"

        #expect(try draft.payload(since: loaded).print_pages == 412)
    }

    @Test func payloadOmitsPrintPagesWhenTheFieldIsBlanked() throws {
        // There is no "empty" integer on the wire, so a blanked field means
        // leave the override alone — not clear it. The web editor's
        // `build_overrides` makes the same choice; the two must agree.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = ""

        #expect(try draft.payload(since: loaded).print_pages == nil)
    }

    @Test func payloadOmitsPrintPagesWhenRetypedUnchanged() throws {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = " 180 "

        #expect(try draft.payload(since: loaded).print_pages == nil)
    }

    @Test func payloadRejectsANonNumericPrintPagesEntry() {
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.printPages = "about 400"

        #expect(throws: MetadataDraftError.invalidPrintPages) {
            try draft.payload(since: loaded)
        }
    }

    @Test func payloadOmitsTheNewFieldsWhenNothingElseChangedThem() throws {
        // AC3, and the merge guarantee this ticket exists to protect: an
        // edit elsewhere must not restate ISBN-10 or the print page count,
        // because `POST /overrides` merges — a client that restates a stale
        // value clobbers whatever the other editor set.
        let loaded = MetadataDraft(book: gatsby())
        var draft = loaded
        draft.title = "Trimalchio"

        let payload = try draft.payload(since: loaded)
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

        let payload = try draft.payload(since: loaded)
        let data = try JSONEncoder().encode(payload)
        let json = try #require(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        #expect(json["title"] as? String == "Trimalchio")
        #expect(!json.keys.contains("isbn10"))
        #expect(!json.keys.contains("print_pages"))
        #expect(!json.keys.contains("genres"))
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
