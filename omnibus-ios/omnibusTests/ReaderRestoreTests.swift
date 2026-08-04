//  ReaderRestoreTests.swift
//  What the reader boots from, and how it comes back from a page WebKit threw
//  away.
//
//  A backgrounded app's web content process is jettisoned routinely, and the
//  page that replaces it re-runs the whole boot — so the boot is not the
//  one-time event it looks like in the code. Booting from the position the
//  book was *opened* at is invisible in a screenshot of a working reader and
//  unmistakable in use: you come back to where you started the session, and
//  the relocate that follows saves it as your progress. Both halves of the
//  recovery are pinned here.

import Foundation
import Testing

@testable import omnibus

private let openedAt = "epubcfi(/6/4!/4/2/2[c01]/1:0)"
private let readTo = "epubcfi(/6/14!/4/2/48[c07]/1:214)"

private func book() -> Book {
    Book(
        id: 7, filename: "hound.epub", title: "The Hound of the Baskervilles",
        uniqueIdentifier: "hound-uuid"
    )
}

private func highlight(id: Int64, cfiRange: String?) -> Highlight {
    Highlight(id: id, bookUUID: "hound-uuid", epubCFIRange: cfiRange, color: .amber, createdAt: 0)
}

private func hostReady() -> [String: Any] { ["type": "hostReady"] }
private func stageReady() -> [String: Any] { ["type": "status", "payload": "ready"] }

/// A `relocate` as the glue posts it — encoded and taken back out through the
/// same decode the controller runs, so a renamed field fails here too.
private func relocate(cfi: String?) throws -> [String: Any] {
    let data = try JSONEncoder().encode(RelocateData(cfi: cfi, page: 214, totalPages: 400, pct: 53))
    return ["type": "relocate", "payload": String(decoding: data, as: UTF8.self)]
}

/// The options object a boot script hands `OmnibusReader.init`. The book URL
/// carries no braces, so the outermost pair delimits the options.
private func bootOptions(_ script: String?) throws -> [String: Any] {
    let script = try #require(script)
    let start = try #require(script.firstIndex(of: "{"))
    let end = try #require(script.lastIndex(of: "}"))
    let json = try #require(String(script[start...end]).data(using: .utf8))
    let object = try JSONSerialization.jsonObject(with: json)
    return try #require(object as? [String: Any])
}

@Suite("Reader boot position")
@MainActor
struct ReaderBootPositionTests {
    @Test("the first boot opens at the position the book was opened at")
    func firstBootUsesTheOpeningPosition() throws {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: openedAt, highlights: [])

        let options = try bootOptions(controller.bootScript())
        #expect(options["cfi"] as? String == openedAt)
    }

    @Test("a book with no saved position boots without a cfi rather than an empty one")
    func noPositionOmitsTheKeyEntirely() throws {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: nil, highlights: [])

        let options = try bootOptions(controller.bootScript())
        #expect(!options.keys.contains("cfi"))
    }

    @Test("a blank saved position is no position")
    func blankPositionIsDropped() throws {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: "  ", highlights: [])

        let options = try bootOptions(controller.bootScript())
        #expect(!options.keys.contains("cfi"))
    }

    @Test("a relocate becomes the position the next boot resumes from")
    func relocateAdvancesTheResumePosition() throws {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: openedAt, highlights: [])
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())

        let moved = try relocate(cfi: readTo)
        controller.handle(message: moved)

        #expect(controller.resumeCFI == readTo)
    }

    /// The glue reports a location before its CFI is resolvable. Taking that
    /// as "no position" would make the next boot open the book at page one.
    @Test("a relocate carrying no cfi leaves the last position standing")
    func relocateWithoutACFIKeepsTheLastPosition() throws {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: openedAt, highlights: [])
        let moved = try relocate(cfi: readTo)
        controller.handle(message: moved)

        let anonymous = try relocate(cfi: nil)
        controller.handle(message: anonymous)

        #expect(controller.resumeCFI == readTo)
    }
}

@Suite("Reader restore after a jettisoned page")
@MainActor
struct ReaderRestoreTests {
    /// The whole bug in one test: read past where you opened, lose the page to
    /// WebKit, and the boot that brings it back has to carry where you got to.
    @Test("a reboot resumes where the reader is, not where the book was opened")
    func rebootResumesAtTheLivePosition() throws {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: openedAt, highlights: [])
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())
        let moved = try relocate(cfi: readTo)
        controller.handle(message: moved)

        controller.webContentProcessDidTerminate()

        let options = try bootOptions(controller.bootScript())
        #expect(options["cfi"] as? String == readTo)
    }

    @Test("a jettisoned page reads as not ready until the one replacing it reports in")
    func terminationHoldsTheStageUntilItBootsAgain() {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: openedAt, highlights: [])
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())
        #expect(controller.isReady)

        controller.webContentProcessDidTerminate()
        #expect(controller.isRestoring)
        #expect(!controller.isReady)

        controller.handle(message: hostReady())
        #expect(!controller.isRestoring)
        controller.handle(message: stageReady())
        #expect(controller.isReady)
    }

    /// The marks are drawn by the page, so they die with it. The set they are
    /// drawn from has to outlive the first paint, or the book comes back
    /// unmarked — and the annotation diff has to know the page is blank, or it
    /// finds nothing to do and repaints nothing.
    @Test("the marks on a jettisoned page are painted onto the one that replaces it")
    func rebootRepaintsTheHighlights() {
        let controller = ReaderController()
        let marks = [
            highlight(id: 1, cfiRange: "epubcfi(/6/4!/4/2,/1:0,/1:8)"),
            highlight(id: 2, cfiRange: "epubcfi(/6/8!/4/6,/1:12,/1:40)"),
        ]
        controller.configure(book: book(), startCFI: openedAt, highlights: marks)
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())
        #expect(controller.drawnHighlights == marks)

        controller.webContentProcessDidTerminate()
        #expect(controller.drawnHighlights.isEmpty)

        controller.handle(message: hostReady())
        controller.handle(message: stageReady())
        #expect(controller.drawnHighlights == marks)
    }

    /// Kobo-origin rows are anchored by an opaque span the reader can't
    /// resolve, so they list but never paint — including through a reboot.
    @Test("an anchorless highlight is never counted as drawn")
    func anchorlessHighlightsAreNotDrawn() {
        let controller = ReaderController()
        let anchored = highlight(id: 1, cfiRange: "epubcfi(/6/4!/4/2,/1:0,/1:8)")
        controller.configure(
            book: book(), startCFI: nil,
            highlights: [anchored, highlight(id: 2, cfiRange: nil)]
        )
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())

        #expect(controller.drawnHighlights == [anchored])
    }

    /// The reader paints its own creates, recolours and deletes one mark at a
    /// time, so the controller learns of them only by being told. A highlight
    /// made this session has to come back with the page like any other.
    @Test("a highlight made this session survives the page being thrown away")
    func locallyMadeHighlightsSurviveAReboot() {
        let controller = ReaderController()
        let opening = highlight(id: 1, cfiRange: "epubcfi(/6/4!/4/2,/1:0,/1:8)")
        controller.configure(book: book(), startCFI: openedAt, highlights: [opening])
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())

        let made = highlight(id: 2, cfiRange: "epubcfi(/6/8!/4/6,/1:12,/1:40)")
        controller.addAnnotation(cfiRange: "epubcfi(/6/8!/4/6,/1:12,/1:40)", color: .amber)
        controller.noteHighlights([opening, made])

        controller.webContentProcessDidTerminate()
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())

        #expect(controller.drawnHighlights == [opening, made])
    }

    /// The reader loads its local highlights before the stage reports ready,
    /// so the note can land first. Taking it as "these are on the page" would
    /// leave the first paint with a diff that finds nothing to do — a book
    /// that opens with no marks at all.
    @Test("marks noted before the stage is ready are still painted on the first paint")
    func notesBeforeReadyDoNotSuppressTheFirstPaint() {
        let controller = ReaderController()
        let marks = [highlight(id: 1, cfiRange: "epubcfi(/6/4!/4/2,/1:0,/1:8)")]
        controller.configure(book: book(), startCFI: openedAt, highlights: marks)
        controller.handle(message: hostReady())

        controller.noteHighlights(marks)
        #expect(controller.drawnHighlights.isEmpty)

        controller.handle(message: stageReady())
        #expect(controller.drawnHighlights == marks)
    }

    /// A set that lands while the stage is down is still the set the reboot
    /// paints — the server's answer doesn't have to wait for the page.
    @Test("highlights arriving while the page is gone are painted when it comes back")
    func highlightsArrivingDuringARestoreAreKept() {
        let controller = ReaderController()
        controller.configure(book: book(), startCFI: nil, highlights: [])
        controller.handle(message: hostReady())
        controller.handle(message: stageReady())

        controller.webContentProcessDidTerminate()
        let late = [highlight(id: 9, cfiRange: "epubcfi(/6/12!/4/2,/1:4,/1:22)")]
        controller.applyHighlights(late)

        controller.handle(message: hostReady())
        controller.handle(message: stageReady())
        #expect(controller.drawnHighlights == late)
    }
}
