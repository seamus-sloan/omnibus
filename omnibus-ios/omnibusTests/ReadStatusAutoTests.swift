//  ReadStatusAutoTests.swift
//  The readers' automatic read-status transitions: the pure decision table,
//  and the tracker's fetch-then-apply sequencing — inert on a failed fetch,
//  settled against repeat observations, never a downgrade.
//
//  The decision table mirrors the web reader's `read_status_auto` tests, so
//  the two clients agree on when a reader may move the chip on its own.

import Foundation
import Testing

@testable import omnibus

@Suite("Read-status auto transition")
struct ReadStatusTransitionTests {
    @Test("an unread book is marked reading on open")
    func unreadBecomesReadingOnOpen() {
        #expect(ReadStatusAuto.transition(current: .unread, atEnd: false) == .reading)
    }

    @Test("reading and finished are left untouched on open")
    func openNeverRewritesStartedBooks() {
        #expect(ReadStatusAuto.transition(current: .reading, atEnd: false) == nil)
        #expect(ReadStatusAuto.transition(current: .finished, atEnd: false) == nil)
    }

    @Test("any unfinished book is marked finished at the end")
    func endFinishesUnfinishedBooks() {
        #expect(ReadStatusAuto.transition(current: .unread, atEnd: true) == .finished)
        #expect(ReadStatusAuto.transition(current: .reading, atEnd: true) == .finished)
    }

    @Test("an already finished book is never rewritten")
    func endNeverRewritesFinished() {
        #expect(ReadStatusAuto.transition(current: .finished, atEnd: true) == nil)
    }
}

@Suite("Read-status auto tracker")
@MainActor
struct ReadStatusAutoTrackerTests {
    /// What the tracker wrote, in order — the whole observable surface.
    private final class Log {
        var writes: [ReadStatus] = []
    }

    private func tracker(stored: ReadStatus?, log: Log) -> ReadStatusAuto {
        ReadStatusAuto(
            fetch: { stored },
            write: { log.writes.append($0) }
        )
    }

    @Test("opening an unread book writes reading once")
    func openWritesReading() async {
        let log = Log()
        let auto = tracker(stored: .unread, log: log)
        await auto.bookOpened()
        #expect(log.writes == [.reading])
    }

    @Test("reaching the end after opening writes finished")
    func endWritesFinished() async {
        let log = Log()
        let auto = tracker(stored: .unread, log: log)
        await auto.bookOpened()
        await auto.positionChanged(atEnd: true)
        #expect(log.writes == [.reading, .finished])
    }

    @Test("a finished book is untouched by opening and re-reaching the end")
    func finishedIsNeverDowngraded() async {
        let log = Log()
        let auto = tracker(stored: .finished, log: log)
        await auto.bookOpened()
        await auto.positionChanged(atEnd: true)
        #expect(log.writes.isEmpty)
    }

    @Test("a failed status fetch keeps every transition inert")
    func failedFetchStaysInert() async {
        let log = Log()
        let auto = tracker(stored: nil, log: log)
        await auto.bookOpened()
        await auto.positionChanged(atEnd: true)
        #expect(log.writes.isEmpty)
    }

    @Test("an end observed before the fetch lands is applied when it does")
    func endBeforeFetchStillFinishes() async {
        // The comic restored onto its last page: the opening position is fed
        // before the status fetch settles.
        let log = Log()
        let auto = tracker(stored: .reading, log: log)
        await auto.positionChanged(atEnd: true)
        #expect(log.writes.isEmpty)
        await auto.bookOpened()
        #expect(log.writes == [.finished])
    }

    @Test("paging back and forth past the end writes finished once")
    func endWritesAreSettled() async {
        let log = Log()
        let auto = tracker(stored: .reading, log: log)
        await auto.bookOpened()
        await auto.positionChanged(atEnd: true)
        await auto.positionChanged(atEnd: false)
        await auto.positionChanged(atEnd: true)
        #expect(log.writes == [.finished])
    }
}
