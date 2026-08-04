//  ReaderRebootTests.swift
//  What the reader hands a page that replaces the one it was reading.
//
//  The epub.js stage is booted more than once per book: iOS reclaims the
//  web-content process of an app that has been in the background a while, and
//  `reader.html` is loaded again when the reader comes back. Every one of those
//  boots reads the same state, so these pin what it has to be — the position the
//  reader actually reached rather than the one the book was opened at, and the
//  marks the outgoing page was holding (#1656).

import Foundation
import Testing

@testable import omnibus

@MainActor
private func openReader(at cfi: String?, holding highlights: [Highlight] = [])
    -> ReaderController
{
    let controller = ReaderController()
    controller.configure(
        book: Book(
            id: 1, filename: "hound.epub", title: "The Hound of the Baskervilles",
            uniqueIdentifier: "book-uuid"
        ),
        startCFI: cfi,
        highlights: highlights
    )
    // What the page announces on load, and what the glue reports once a restore
    // has settled — the sequence every open goes through.
    controller.handle(message: ["type": "hostReady"])
    controller.handle(message: ["type": "status", "payload": "ready"])
    return controller
}

/// A relocate as the glue posts it: `buildRelocateData`, JSON, over the bridge.
@MainActor
private func relocate(_ controller: ReaderController, to cfi: String) throws {
    let data = RelocateData(
        cfi: cfi, page: 212, totalPages: 661, pct: 32,
        chapter: 7, totalChapters: 24, chapterTitle: "The Moor", chapterPagesLeft: 9
    )
    let json = String(decoding: try JSONEncoder().encode(data), as: UTF8.self)
    controller.handle(message: ["type": "relocate", "payload": json])
}

/// The options a boot hands the glue, read back out of the `init` call.
///
/// Parsed rather than string-matched because the CFI goes over as JSON, where
/// every `/` in it is escaped — a literal comparison against the script text
/// would be testing the encoder, not the position.
@MainActor
private func bootOptions(_ controller: ReaderController) throws -> [String: Any] {
    let script = try #require(controller.bootScript())
    // `OmnibusReader.init('stage', "…", { … })` — the options are the last
    // argument, so from the first brace to the closing paren.
    let brace = try #require(script.firstIndex(of: "{"))
    let object = try JSONSerialization.jsonObject(with: Data(script[brace...].dropLast().utf8))
    return try #require(object as? [String: Any])
}

private func mark(_ range: String) -> Highlight {
    Highlight(
        id: 1, bookUUID: "book-uuid", epubCFIRange: range, color: .amber,
        note: nil, text: "a passage", clientID: nil, createdAt: 0
    )
}

private let openedAt = "epubcfi(/6/14[chap07]!/4/2/2,/1:0,/1:1)"
private let readTo = "epubcfi(/6/14[chap07]!/4/2/58,/1:0,/1:1)"

@Suite("Reader page reboot")
@MainActor
struct ReaderRebootTests {
    @Test("the restore point follows the reader, not the position the book opened at")
    func restorePointFollowsTheReader() throws {
        let controller = openReader(at: openedAt)
        try relocate(controller, to: readTo)

        #expect(controller.restoreCFI == readTo)
    }

    /// The bug itself. A page reloaded under the reader used to boot from the
    /// CFI `configure` was given at open, so an hour's reading was undone by a
    /// backgrounding — and the relocate that followed the restore persisted the
    /// revert and pushed it to every other device.
    @Test("a reloaded page boots where the reader is, not where the book was opened")
    func reloadBootsAtTheLivePosition() throws {
        let controller = openReader(at: openedAt)
        try relocate(controller, to: readTo)

        // WebKit reclaimed the process and loaded `reader.html` again.
        controller.handle(message: ["type": "hostReady"])

        #expect(try bootOptions(controller)["cfi"] as? String == readTo)
    }

    @Test("a book opened with no saved position still boots from wherever it was read to")
    func reloadCarriesAPositionAFreshBookHadNone() throws {
        let controller = openReader(at: nil)
        // Nothing to restore to on the first boot: the book opens at page one.
        #expect(try bootOptions(controller)["cfi"] == nil)

        try relocate(controller, to: readTo)
        controller.handle(message: ["type": "hostReady"])

        #expect(try bootOptions(controller)["cfi"] as? String == readTo)
    }

    @Test("a blank cfi in a relocate leaves the restore point standing")
    func blankRelocateDoesNotClearTheRestorePoint() throws {
        let controller = openReader(at: openedAt)
        try relocate(controller, to: readTo)

        controller.handle(message: ["type": "relocate", "payload": #"{"cfi":""}"#])

        #expect(controller.restoreCFI == readTo)
    }

    @Test("a reload puts the marks the old page was holding back on the queue")
    func reloadRequeuesTheMarks() throws {
        let controller = openReader(at: openedAt, holding: [mark("epubcfi(/6/14!/4/2/4,/1:0,/1:9)")])
        // Painted, and the queue drained, by the `ready` that opened the book.
        #expect(controller.pendingHighlights.isEmpty)

        controller.handle(message: ["type": "hostReady"])

        // Otherwise the reader comes back to a book with every highlight gone
        // until it is closed and opened again.
        #expect(controller.pendingHighlights.count == 1)
    }

    @Test("a reload holds the reader unready so the loading overlay covers it")
    func reloadDropsBackToNotReady() {
        let controller = openReader(at: openedAt)
        #expect(controller.isReady)

        controller.handle(message: ["type": "hostReady"])

        // A stage whose process is gone paints nothing; left `ready`, the reader
        // shows a blank page at full opacity instead of "Opening …".
        #expect(!controller.isReady)
    }

    @Test("a terminated web-content process leaves the reader ready for the page that replaces it")
    func terminationHandsOverBeforeTheReload() throws {
        let controller = openReader(at: openedAt, holding: [mark("epubcfi(/6/14!/4/2/4,/1:0,/1:9)")])
        try relocate(controller, to: readTo)

        controller.webContentProcessDidTerminate()

        #expect(!controller.isReady)
        #expect(controller.pendingHighlights.count == 1)
        #expect(try bootOptions(controller)["cfi"] as? String == readTo)
    }
}
