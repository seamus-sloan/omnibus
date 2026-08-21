//  ReaderEchoTests.swift
//  What "echo" on a relocate means for persistence and the restore anchor.
//
//  A relocate that merely re-states a position the reader already reported —
//  the boot restore settling, or a rotation/resize's corrected re-pagination
//  (#2081) — carries `echo: true`. It must still reach the page (rendered
//  like any relocate), but must never move `restoreCFI`: moving the anchor
//  for a rotation would reboot the page onto a spread-short position the
//  reader was never actually shown. `RelocateData.isMovement` is the same
//  flag ReaderView's persist guard reads — a SwiftUI `onChange` closure
//  isn't unit-testable in isolation, so the "not persisted" half is pinned
//  at that guard's input rather than the write it skips.

import Foundation
import Testing

@testable import omnibus

@MainActor
private func openReader(at cfi: String?) -> ReaderController {
    let controller = ReaderController()
    controller.configure(
        book: Book(
            id: 1, filename: "hound.epub", title: "The Hound of the Baskervilles",
            uniqueIdentifier: "book-uuid"
        ),
        startCFI: cfi,
        highlights: []
    )
    // What the page announces on load, and what the glue reports once a
    // restore has settled — the sequence every open goes through.
    controller.handle(message: ["type": "hostReady"])
    controller.handle(message: ["type": "status", "payload": "ready"])
    return controller
}

/// A relocate as the glue posts it: `buildRelocateData`, JSON, over the
/// bridge — with an explicit `echo` flag, as a rotation's corrected landing
/// carries it.
@MainActor
private func relocate(_ controller: ReaderController, to cfi: String, echo: Bool = false) throws {
    let data = RelocateData(
        cfi: cfi, page: 212, totalPages: 661, pct: 32,
        chapter: 7, totalChapters: 24, chapterTitle: "The Moor", chapterPagesLeft: 9,
        echo: echo
    )
    let json = String(decoding: try JSONEncoder().encode(data), as: UTF8.self)
    controller.handle(message: ["type": "relocate", "payload": json])
}

private let openedAt = "epubcfi(/6/14[chap07]!/4/2/2,/1:0,/1:1)"
private let readTo = "epubcfi(/6/14[chap07]!/4/2/58,/1:0,/1:1)"
// One column short of `readTo` — the exact shape of the drift finding 1
// describes: a rotation's corrected redisplay lands the settled viewport a
// spread away from the position it was asked to restate.
private let rotatedTo = "epubcfi(/6/14[chap07]!/4/2/56,/1:0,/1:1)"

@Suite("Reader relocate echo")
@MainActor
struct ReaderEchoTests {
    @Test("a relocate with echo unset is movement")
    func defaultRelocateIsMovement() {
        let data = RelocateData(cfi: readTo)

        #expect(data.echo == false)
        #expect(data.isMovement)
    }

    @Test("a relocate tagged echo is not movement")
    func echoTaggedRelocateIsNotMovement() {
        let data = RelocateData(cfi: readTo, echo: true)

        #expect(data.echo)
        #expect(!data.isMovement)
    }

    /// The bug itself, at the restore anchor. A rotation's own corrected
    /// redisplay reaches the page normally — the reader sees the settled
    /// landing — but must not become where a reboot picks up.
    @Test("an echoed relocate reaches the page but leaves the restore point standing")
    func echoedRelocateDoesNotMoveTheRestorePoint() throws {
        let controller = openReader(at: openedAt)
        try relocate(controller, to: readTo)
        #expect(controller.restoreCFI == readTo)

        // A rotation's corrected re-pagination: the glue reports the settled
        // landing tagged `echo: true`.
        try relocate(controller, to: rotatedTo, echo: true)

        // Rendered — the page indicator/footer follow it like any relocate.
        #expect(controller.location?.cfi == rotatedTo)
        #expect(controller.location?.echo == true)
        // Not movement — a reboot still lands where the reader actually was
        // before the rotation, not the drifted echo.
        #expect(controller.restoreCFI == readTo)
    }

    @Test("a real page turn after an echo still moves the restore point")
    func realMovementAfterAnEchoStillMovesTheRestorePoint() throws {
        let controller = openReader(at: openedAt)
        try relocate(controller, to: readTo)
        try relocate(controller, to: rotatedTo, echo: true)

        let turnedTo = "epubcfi(/6/16[chap08]!/4/2/2,/1:0,/1:1)"
        try relocate(controller, to: turnedTo)

        #expect(controller.restoreCFI == turnedTo)
    }
}
