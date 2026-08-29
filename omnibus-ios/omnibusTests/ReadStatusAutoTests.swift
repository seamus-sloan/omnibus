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

    private func tracker(
        stored: ReadStatus?, log: Log, online: Bool = true
    ) -> ReadStatusAuto {
        ReadStatusAuto(
            fetch: { stored },
            write: { log.writes.append($0) },
            isOnline: { online }
        )
    }

    /// A device whose connection and whose answer both move during a test:
    /// the offline open that learns nothing, and the moment one of the two
    /// comes back. Counts fetches, so "does not ask again" is assertable.
    private final class Device {
        var online = false
        var answer: ReadStatus?
        private(set) var fetches = 0

        func fetch() -> ReadStatus? {
            fetches += 1
            return answer
        }
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

    @Test("an end observed before the fetch lands settles it with one write")
    func endBeforeFetchStillFinishes() async {
        // The comic restored onto its last page: the opening position is fed
        // before the status fetch settles. Reaching the end pulls the status
        // itself rather than waiting, and the later open must not re-run the
        // transition it already applied.
        let log = Log()
        let auto = tracker(stored: .reading, log: log)
        await auto.positionChanged(atEnd: true)
        #expect(log.writes == [.finished])
        await auto.bookOpened()
        #expect(log.writes == [.finished])
    }

    @Test("the player finishes a book it never opened through the tracker")
    func endWithoutOpenStillFinishes() async {
        // The audio player reaches end-of-book off `AVPlayerItemDidPlayToEnd`,
        // which can fire on a load whose `play()` — and so whose `bookOpened`
        // — never ran (a lock-screen resume, an already-loaded book). Losing
        // the completion there would leave a fully-listened book unfinished.
        let log = Log()
        let auto = tracker(stored: .reading, log: log)
        await auto.positionChanged(atEnd: true)
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

    @Test("a status that could not be fetched at open is written once it can be")
    func unknownStatusIsRetriedNotLatched() async {
        // The reported case: the whole read happened offline, so the opening
        // fetch had no server to ask and no replica row to fall back on.
        // Latching there left a book read to 10% with no status at all (#2289).
        let log = Log()
        let device = Device()
        let auto = ReadStatusAuto(
            fetch: { device.fetch() },
            write: { log.writes.append($0) },
            isOnline: { device.online }
        )

        await auto.bookOpened()
        #expect(log.writes.isEmpty)

        // Turning pages offline must not cost a request per page.
        await auto.positionChanged(atEnd: false)
        await auto.positionChanged(atEnd: false)
        #expect(device.fetches == 1)

        device.online = true
        device.answer = .unread
        await auto.positionChanged(atEnd: false)
        #expect(log.writes == [.reading])

        // Having settled, it stops asking.
        await auto.positionChanged(atEnd: false)
        #expect(device.fetches == 2)
    }

    @Test("reaching the end retries the status even while offline")
    func endRetriesEvenWhileOffline() async {
        // Unlike the open transition, finishing cannot downgrade anything, so
        // it is worth an attempt whatever the connection is doing — offline
        // the replica may answer it, which a queued write makes authoritative.
        let log = Log()
        let device = Device()
        let auto = ReadStatusAuto(
            fetch: { device.fetch() },
            write: { log.writes.append($0) },
            isOnline: { device.online }
        )

        await auto.bookOpened()
        await auto.positionChanged(atEnd: false)
        #expect(device.fetches == 1)

        device.answer = .reading
        await auto.positionChanged(atEnd: true)
        #expect(device.fetches == 2)
        #expect(log.writes == [.finished])
    }
}

/// The wire contract the auto transitions decide against.
///
/// `GET /api/read-status/{uuid}` answers `200 null` for a book nobody has
/// marked, which is an answer and not a failure. Decoding the record
/// non-optionally collapsed the two, so an unmarked book never reached the
/// replica however often it was read online — and the readers, which fall back
/// to that replica when the server can't be asked, had nothing to decide
/// against offline (#2289).
@Suite("Read-status wire contract")
struct ReadStatusRecordCodecTests {
    private let decoder = JSONDecoder()

    @Test("a null body decodes as no row rather than a decode failure")
    func nullDecodesToNoRow() throws {
        let record = try decoder.decode(
            ReadStatusRecord?.self, from: Data("null".utf8)
        )
        #expect(record == nil)
    }

    @Test("no row stands for unread, with both clocks unset")
    func noRowIsUnread() {
        let record = ReadStatusRecord.unmarked(uuid: "book-1")
        #expect(record.status == .unread)
        #expect(record.bookUUID == "book-1")
        #expect(record.updatedAt == 0)
        #expect(record.finishedAt == nil)
    }

    @Test("a real row decodes over the server's snake_case keys")
    func rowDecodes() throws {
        let json = """
        {"book_uuid":"book-1","status":"reading","updated_at":1700000000,
         "finished_at":null}
        """
        let record = try decoder.decode(
            ReadStatusRecord?.self, from: Data(json.utf8)
        )
        #expect(record?.status == .reading)
        #expect(record?.updatedAt == 1_700_000_000)
    }
}
