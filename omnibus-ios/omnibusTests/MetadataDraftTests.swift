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
    book.series = "—"
    book.publisher = "Scribner"
    book.language = "en"
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
}

struct ChipEntryTests {
    @Test func committedTrimsSurroundingWhitespace() {
        let chip = ChipEntry.committed(from: "  Jazz Age  ", existing: [], deduplicating: true)
        #expect(chip == "Jazz Age")
    }

    @Test func committedRefusesABlankEntry() {
        #expect(ChipEntry.committed(from: "   ", existing: [], deduplicating: false) == nil)
        #expect(ChipEntry.committed(from: "", existing: [], deduplicating: true) == nil)
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
