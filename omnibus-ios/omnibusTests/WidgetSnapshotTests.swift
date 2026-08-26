//  WidgetSnapshotTests.swift
//  The two contracts between the app and its widget extension.
//
//  Both are contracts across a process boundary that nothing type-checks: the
//  app writes a snapshot and builds a URL, the extension reads one and the app
//  parses the other back, and a mismatch surfaces as a blank tile or a tap that
//  opens the app at the wrong screen — with nothing logged on either side.

import Foundation
import Testing

@testable import omnibus

private func entry(
    uuid: String = "book-1",
    format: WidgetFormat = .epub,
    fraction: Double? = 0.42,
    secondsRemaining: Double? = nil,
    fileID: Int64? = nil,
    thumb: String? = "book-1.jpg"
) -> WidgetBook {
    WidgetBook(
        bookUUID: uuid,
        format: format,
        title: "Immersive Voyage",
        author: "Ada Marlowe",
        tone: WidgetBook.Tone(l: 0.55, c: 0.11, h: 42),
        fraction: fraction,
        secondsRemaining: secondsRemaining,
        // Whole seconds: a progress row's `updated_at` is one, and the wire
        // form encodes dates as seconds since the epoch.
        updatedAt: Date(timeIntervalSince1970: 1_724_500_000),
        fileID: fileID,
        thumb: thumb
    )
}

@Suite("Widget snapshot wire form")
struct WidgetSnapshotCodableTests {
    @Test
    func snapshot_round_trips_through_the_wire_form_the_extension_reads() throws {
        let original = WidgetSnapshot(
            state: .ready,
            books: [
                entry(),
                entry(
                    uuid: "book-2", format: .audio, fraction: 0.61,
                    secondsRemaining: 4 * 3600 + 12 * 60, fileID: 7, thumb: "book-2.jpg"
                ),
            ],
            generatedAt: Date(timeIntervalSince1970: 1_724_500_500)
        )

        let decoded = try WidgetStore.decode(WidgetStore.encode(original))

        #expect(decoded == original)
    }

    @Test
    func snapshot_round_trips_an_entry_whose_optional_fields_are_all_absent() throws {
        // A CFI-only EPUB save carries no percentage, a coverless book no
        // thumb, and neither carries a file id — the shape most likely to be
        // dropped by a hand-written encoder.
        let original = WidgetSnapshot(
            state: .ready,
            books: [entry(fraction: nil, secondsRemaining: nil, fileID: nil, thumb: nil)],
            generatedAt: Date(timeIntervalSince1970: 1_724_500_500)
        )

        let decoded = try WidgetStore.decode(WidgetStore.encode(original))

        #expect(decoded == original)
        #expect(decoded.books[0].fraction == nil)
        #expect(decoded.books[0].thumb == nil)
    }

    @Test(arguments: [
        WidgetSnapshot.State.signedOut,
        .emptyLibrary,
        .nothingInProgress,
        .ready,
    ])
    func snapshot_round_trips_every_empty_state(state: WidgetSnapshot.State) throws {
        let original = WidgetSnapshot(state: state, generatedAt: .init(timeIntervalSince1970: 1))

        #expect(try WidgetStore.decode(WidgetStore.encode(original)) == original)
    }

    @Test
    func snapshot_entry_identity_separates_the_two_formats_of_one_book() {
        // Mirrors `ResumePoint.id`: a book someone is both reading and
        // listening to is two cards, and one identity would let a `ForEach`
        // shuffle their state between them.
        #expect(entry(format: .epub).id != entry(format: .audio).id)
    }

    @Test
    func thumb_name_cannot_escape_the_shared_container() {
        // The uuid arrives from the server; a separator smuggled into one
        // would write outside the App Group.
        #expect(!WidgetStore.thumbName(for: "../../etc/passwd").contains("/"))
    }
}

@Suite("Widget deep links")
struct WidgetDeepLinkTests {
    @Test
    func link_round_trips_a_book_with_a_format_and_a_file() {
        let original = DeepLink.book(uuid: "abc-123", format: .audio, fileID: 12)

        #expect(DeepLink(original.url) == original)
    }

    @Test
    func link_round_trips_a_book_carrying_neither_format_nor_file() {
        let original = DeepLink.book(uuid: "abc-123", format: nil, fileID: nil)

        #expect(DeepLink(original.url) == original)
        // Nothing to say, so nothing said — a bare `?` would still parse, but
        // it is not what the widget mints.
        #expect(original.url.absoluteString == "omnibus://book/abc-123")
    }

    @Test
    func link_round_trips_a_uuid_needing_percent_encoding() {
        let original = DeepLink.book(uuid: "a b/c", format: .epub, fileID: nil)

        #expect(DeepLink(original.url) == original)
    }

    @Test(arguments: [
        // Another app's scheme.
        "https://book/abc-123",
        // Our scheme, a host we don't route.
        "omnibus://author/4",
        // No book named at all.
        "omnibus://book",
        "omnibus://book/",
        // More than one path segment is not a link this app mints.
        "omnibus://book/abc-123/extra",
    ])
    func link_refuses_a_url_this_app_never_minted(raw: String) {
        #expect(DeepLink(URL(string: raw)!) == nil)
    }

    @Test
    func link_drops_a_format_it_does_not_recognise_rather_than_refusing_the_tap() {
        // A widget built against a newer format must still open the book; the
        // app resolves one from the book's own formats when none is named.
        let parsed = DeepLink(URL(string: "omnibus://book/abc-123?format=braille")!)

        #expect(parsed == .book(uuid: "abc-123", format: nil, fileID: nil))
    }

    @Test
    func link_drops_a_file_id_that_is_not_a_number() {
        let parsed = DeepLink(URL(string: "omnibus://book/abc-123?format=audio&file=none")!)

        #expect(parsed == .book(uuid: "abc-123", format: .audio, fileID: nil))
    }
}

@Suite("Resume format resolution")
struct ResumeFormatTests {
    private func book(formats: [String]) -> Book {
        Book(id: 1, filename: "b.epub", title: "B", uniqueIdentifier: "b", formats: formats)
    }

    @Test
    func requested_format_is_honoured_when_the_book_still_carries_it() {
        #expect(book(formats: ["epub", "m4b"]).resumeFormat(for: .audio) == .audio)
        #expect(book(formats: ["epub", "m4b"]).resumeFormat(for: .epub) == .epub)
    }

    @Test
    func audio_position_on_a_book_whose_audiobook_is_gone_resumes_into_the_reader() {
        // A progress row soft-references `books.uuid` with no cascade, so it
        // outlives the file it was taken in — offering "Play" for one opens a
        // player with nothing to play.
        #expect(book(formats: ["epub"]).resumeFormat(for: .audio) == .epub)
    }

    @Test
    func reading_position_on_an_audio_only_book_resumes_into_the_player() {
        #expect(book(formats: ["m4b"]).resumeFormat(for: .epub) == .audio)
    }

    @Test
    func a_payload_that_omitted_the_formats_list_defers_to_the_request() {
        // Empty means "not projected", not "the book has none".
        #expect(book(formats: []).resumeFormat(for: .audio) == .audio)
        #expect(book(formats: []).resumeFormat(for: .epub) == .epub)
    }

    @Test
    func a_deep_link_naming_no_format_reads_as_a_request_to_read() {
        let dual = book(formats: ["epub", "m4b"])

        #expect(dual.resumeFormat(for: WidgetFormat?.none) == .epub)
        // …but an audio-only book still lands in the player rather than
        // opening a reader with no file behind it.
        #expect(book(formats: ["m4b"]).resumeFormat(for: WidgetFormat?.none) == .audio)
    }

    @Test
    func the_two_format_enums_agree_on_their_raw_values() {
        // The snapshot and the progress row name the same thing, and the
        // bridging inits fall back to `.epub` rather than trapping — so a
        // divergence would be silent without this.
        #expect(ProgressFormat(WidgetFormat.audio) == .audio)
        #expect(ProgressFormat(WidgetFormat.epub) == .epub)
        #expect(WidgetFormat(ProgressFormat.audio) == .audio)
        #expect(WidgetFormat(ProgressFormat.epub) == .epub)
    }
}
