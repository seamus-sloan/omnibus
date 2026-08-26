//  ReaderRebootTests.swift
//  What the reader hands to the page that replaces the one it was reading.
//
//  The epub.js stage is booted more than once per book: iOS reclaims the
//  web-content process of an app that has been in the background a while, and
//  `reader.html` is loaded again when the reader comes back. Every one of those
//  boots reads the same state, so these pin what it has to be — the position the
//  reader actually reached rather than the one the book was opened at (#1656),
//  the marks the outgoing page was holding, and the typography in force by the
//  time the replacement paints rather than when it started booting (#2191).

import Foundation
import Testing
import UIKit
import WebKit

@testable import omnibus

@MainActor
private func openReader(at cfi: String?, holding highlights: [Highlight] = [])
    -> ReaderController
{
    // Explicit settings, so nothing here depends on the host's stored blob — and
    // a real web view, because the settings handover is gated on a live page.
    let controller = ReaderController(settings: ReaderSettings())
    controller.webView = WKWebView()
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

/// The call a page has to receive for a size change to have actually landed.
private let setFontSize26 = "OmnibusReader.setFontSize(26)"

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

    /// A progress row can hold a blank position — `localProgress` hands it
    /// straight over, unlike the remote read, which normalizes it away.
    @Test("a book opened on a blank saved position boots with no cfi at all")
    func blankSavedPositionIsNoPosition() throws {
        let controller = openReader(at: "")

        #expect(controller.restoreCFI == nil)
        #expect(try bootOptions(controller)["cfi"] == nil)
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

        controller.webContentProcessDidTerminate(state: .active)

        #expect(!controller.isReady)
        #expect(controller.pendingHighlights.count == 1)
        #expect(try bootOptions(controller)["cfi"] as? String == readTo)
    }

    /// Reloading in the background only feeds a fresh process to the same
    /// reclaim — and re-reads the whole book to do it.
    @Test("a process lost while the app is away leaves the reload owed until it returns")
    func reloadIsDeferredPastTheBackground() {
        let controller = openReader(at: openedAt)

        controller.webContentProcessDidTerminate(state: .background)
        #expect(controller.awaitingReload)

        // Still away: a background refresh pass must not spend it either.
        controller.reloadIfNeeded(state: .background)
        #expect(controller.awaitingReload)

        controller.reloadIfNeeded(state: .active)
        #expect(!controller.awaitingReload)
    }

    /// The callback can land either side of the reader's own resume hook, so
    /// both have to be able to take the reload — and only one of them may.
    @Test("a process lost as the app returns is reloaded without waiting for the resume hook")
    func reloadIsTakenWhenTheAppIsAlreadyBack() {
        let controller = openReader(at: openedAt)

        controller.webContentProcessDidTerminate(state: .inactive)

        #expect(!controller.awaitingReload)
    }

    @Test("a page WebKit reloaded on its own leaves nothing owed")
    func webKitsOwnReloadClearsTheDebt() {
        let controller = openReader(at: openedAt)
        controller.webContentProcessDidTerminate(state: .background)

        controller.handle(message: ["type": "hostReady"])

        // Otherwise the resume hook loads the page a second time, throwing away
        // the one that just came back and re-reading the book to do it.
        #expect(!controller.awaitingReload)
    }

    // MARK: - Typography handover

    /// The bug. A boot bakes a *snapshot* of the settings into its options, and
    /// a page rebooting after a backgrounding is not ready for seconds — long
    /// enough for the reader to change a setting in a sheet that is still open
    /// over it. Dropped, the change sat on disk while the page went on
    /// rendering the snapshot, so the sheet read 26 and the page read 19.
    @Test("a size changed while the page was rebooting reaches it when the page comes back")
    func settingsChangedDuringARebootAreAppliedOnReady() {
        withCleanReaderSettings {
            let controller = openReader(at: openedAt)
            // WebKit reclaimed the process: this is the replacement's snapshot.
            controller.handle(message: ["type": "hostReady"])

            controller.settings.fontSize = 26

            // Nothing can take it yet — the replacement page hasn't painted.
            #expect(controller.appliedSettings?.fontSize == ReaderSettings().fontSize)
            #expect(!controller.evaluatedScripts.contains(setFontSize26))

            controller.handle(message: ["type": "status", "payload": "ready"])

            // The bookkeeping is not the fix — the page being told is. Asserting
            // only `appliedSettings` would let the emission be deleted outright
            // with this test still green.
            #expect(controller.evaluatedScripts.contains(setFontSize26))
            #expect(controller.appliedSettings?.fontSize == 26)
        }
    }

    @Test("a size changed while the page is up is handed over as it happens")
    func settingsChangedWhileReadyAreAppliedImmediately() {
        withCleanReaderSettings {
            let controller = openReader(at: openedAt)

            controller.settings.fontSize = 26

            #expect(controller.evaluatedScripts.contains(setFontSize26))
            #expect(controller.appliedSettings?.fontSize == 26)
        }
    }

    /// The other half of the window, carried by `bootScript` reading the settings
    /// live rather than by the sync on ready — a different mechanism, and the
    /// more commonly hit one. Hoisting the boot options into a value captured at
    /// `configure` time (the mistake #1656 fixed for `restoreCFI`) would
    /// reintroduce the reported symptom with every other test still passing.
    @Test("a size changed before the replacement page boots rides its boot options")
    func settingsChangedBeforeHostReadyRideTheBootSnapshot() throws {
        try withCleanReaderSettings {
            let controller = openReader(at: openedAt)
            controller.webContentProcessDidTerminate(state: .active)

            controller.settings.fontSize = 26

            let options = try bootOptions(controller)
            #expect(options["fontSize"] as? Int == 26)
        }
    }

    /// A sync is a diff, not a replay: re-sending a setting the page already has
    /// costs a re-pagination for nothing.
    @Test("a sync sends only what the page is not already showing")
    func syncSendsOnlyTheDifference() {
        var wanted = ReaderSettings()
        wanted.fontSize = 26

        #expect(
            ReaderController.settingsScripts(from: ReaderSettings(), to: wanted)
                == [setFontSize26]
        )
        #expect(ReaderController.settingsScripts(from: wanted, to: wanted).isEmpty)
    }

    @Test("a torn-down page stops being a diff base")
    func teardownDropsTheDiffBase() {
        withCleanReaderSettings {
            let controller = openReader(at: openedAt)
            #expect(controller.appliedSettings != nil)

            controller.teardown()

            // Left standing, the next change would diff against a page that is
            // gone and mark itself applied without ever having been sent.
            #expect(controller.appliedSettings == nil)
            controller.settings.fontSize = 26
            #expect(controller.appliedSettings == nil)
            #expect(!controller.evaluatedScripts.contains(setFontSize26))
        }
    }

    /// The page posts `hostReady` before it goes away; the message lands on the
    /// main queue after the reader has already torn down.
    @Test("a hostReady arriving after teardown does not re-arm the diff base")
    func lateHostReadyDoesNotRearmAfterTeardown() {
        withCleanReaderSettings {
            let controller = openReader(at: openedAt)
            controller.teardown()

            controller.handle(message: ["type": "hostReady"])

            // Re-arming here would undo the teardown: a later change would diff
            // against a page that does not exist and record itself as sent.
            #expect(controller.appliedSettings == nil)
        }
    }
}
