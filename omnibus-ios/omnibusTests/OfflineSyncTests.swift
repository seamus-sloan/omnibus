//  OfflineSyncTests.swift
//  The pure parts of the offline layer: how a queued body is re-encoded for
//  replay, which replica keys a pending write holds off, and what counts as a
//  reachability signal.
//
//  All three are pure functions with no store or network behind them, which is
//  the whole reason they're worth pinning here — each has already been wrong
//  once in a way no screen made obvious.

import Foundation
import Testing

@testable import omnibus

// MARK: - Outbox body re-encoding

/// Round-trip a queued body the way `SyncEngine.replay` does.
private func replayed(_ json: String) throws -> String {
    let data = try JSONEncoder().encode(RawJSON(data: Data(json.utf8)))
    return String(decoding: data, as: UTF8.self)
}

/// Decode a replayed body back to compare a single field without depending on
/// key order, which `JSONEncoder` does not preserve for dictionaries.
private func replayedValue(_ json: String, key: String) throws -> Any? {
    let round = try replayed(json)
    let object = try JSONSerialization.jsonObject(with: Data(round.utf8)) as? [String: Any]
    return object?[key]
}

@Suite("Outbox body re-encoding")
struct RawJSONTests {
    @Test("one-star rating survives replay as a number, not a boolean")
    func oneStarRatingStaysNumeric() throws {
        let stars = try #require(try replayedValue(#"{"stars":1.0}"#, key: "stars") as? NSNumber)
        #expect(CFGetTypeID(stars) != CFBooleanGetTypeID())
        #expect(stars.doubleValue == 1)
    }

    @Test("zero survives replay as a number, not a boolean")
    func zeroStaysNumeric() throws {
        let value = try #require(try replayedValue(#"{"progress":0}"#, key: "progress") as? NSNumber)
        #expect(CFGetTypeID(value) != CFBooleanGetTypeID())
        #expect(value.doubleValue == 0)
    }

    @Test("default playback rate survives replay as a number")
    func defaultPlaybackRateStaysNumeric() throws {
        let value = try #require(
            try replayedValue(#"{"playback_rate":1.0}"#, key: "playback_rate") as? NSNumber
        )
        #expect(CFGetTypeID(value) != CFBooleanGetTypeID())
        #expect(value.doubleValue == 1)
    }

    @Test("genuine booleans still replay as booleans")
    func booleansArePreserved() throws {
        let yes = try #require(try replayedValue(#"{"flag":true}"#, key: "flag") as? NSNumber)
        #expect(CFGetTypeID(yes) == CFBooleanGetTypeID())
        #expect(yes.boolValue)

        let no = try #require(try replayedValue(#"{"flag":false}"#, key: "flag") as? NSNumber)
        #expect(CFGetTypeID(no) == CFBooleanGetTypeID())
        #expect(!no.boolValue)
    }

    @Test("whole numbers stay integral so the server's i64 fields parse")
    func wholeNumbersStayIntegral() throws {
        #expect(try replayed(#"{"started_at":1700000000}"#) == #"{"started_at":1700000000}"#)
    }

    @Test("fractional numbers keep their fraction")
    func fractionalNumbersArePreserved() throws {
        let value = try #require(try replayedValue(#"{"stars":4.5}"#, key: "stars") as? NSNumber)
        #expect(value.doubleValue == 4.5)
    }

    @Test("nulls, strings, and nesting survive a round trip")
    func compoundBodiesArePreserved() throws {
        let body = #"{"book_uuid":"abc","title":null,"tags":["a","b"]}"#
        let object = try #require(
            try JSONSerialization.jsonObject(with: Data(replayed(body).utf8)) as? [String: Any]
        )
        #expect(object["book_uuid"] as? String == "abc")
        #expect(object["title"] is NSNull)
        #expect(object["tags"] as? [String] == ["a", "b"])
    }
}

// MARK: - Revalidation scoping

@Suite("Outbox scope")
struct OutboxScopeTests {
    private let uuid = "11111111-2222-3333-4444-555555555555"

    @Test("a queued position blocks only that book, that format, and the resume rail")
    func progressBlocksItsOwnKeys() {
        let kinds: Set<String> = [OpKind.progress(uuid, .epub)]
        #expect(OutboxScope.isBlocked(CacheKey.progress(uuid, .epub), by: kinds))
        #expect(OutboxScope.isBlocked(CacheKey.recentProgress, by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.progress(uuid, .audio), by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.progress("other", .epub), by: kinds))
    }

    @Test("a queued session report holds off stats and nothing else")
    func sessionBlocksOnlyStats() {
        let kinds: Set<String> = [OpKind.session]
        #expect(OutboxScope.isBlocked(CacheKey.stats(.week), by: kinds))
        // The freeze this scoping exists to prevent: one stuck report used to
        // hold every read in the app on the replica.
        #expect(!OutboxScope.isBlocked(CacheKey.shelves, by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.library, by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.me, by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.highlights(uuid), by: kinds))
    }

    @Test("a queued shelf change holds off every shelf surface")
    func shelfMembershipBlocksShelfKeys() {
        let kinds: Set<String> = [OpKind.shelfMembership]
        #expect(OutboxScope.isBlocked(CacheKey.shelves, by: kinds))
        #expect(OutboxScope.isBlocked(CacheKey.shelfPreviews, by: kinds))
        #expect(OutboxScope.isBlocked(CacheKey.shelf(7), by: kinds))
        #expect(OutboxScope.isBlocked(CacheKey.shelfPage(7), by: kinds))
        #expect(OutboxScope.isBlocked(CacheKey.shelvesContaining(uuid), by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.bookmarks(uuid), by: kinds))
    }

    @Test("book-scoped kinds name exactly one key each")
    func bookScopedKindsAreExact() {
        #expect(OutboxScope.isBlocked(CacheKey.rating(uuid), by: [OpKind.rating(uuid)]))
        #expect(!OutboxScope.isBlocked(CacheKey.rating("other"), by: [OpKind.rating(uuid)]))
        #expect(OutboxScope.isBlocked(CacheKey.readStatus(uuid), by: [OpKind.readStatus(uuid)]))
        // The one pair whose spellings differ — the cache key predates the kind.
        #expect(
            OutboxScope.isBlocked(CacheKey.playbackRate(uuid), by: [OpKind.playbackRate(uuid)])
        )
        #expect(!OutboxScope.isBlocked(CacheKey.rating(uuid), by: [OpKind.playbackRate(uuid)]))
    }

    @Test("a queued read status holds off the stats summary it feeds")
    func readStatusBlocksStats() {
        // `FINISHED_EVENTS` in `db/src/stats/mod.rs` counts a book finished
        // two ways — a 100% journal entry and an explicit read status — and
        // only the journal side used to declare it here, so marking a book
        // finished offline left the stats screen free to read back the
        // pre-change count from a server that had not seen the change.
        let kinds: Set<String> = [OpKind.readStatus(uuid)]
        #expect(OutboxScope.isBlocked(CacheKey.stats(.year), by: kinds))
        #expect(OutboxScope.isBlocked(CacheKey.readStatus(uuid), by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.readStatus("other"), by: kinds))
        #expect(!OutboxScope.isBlocked(CacheKey.shelves, by: kinds))
    }

    @Test("annotation kinds hold off their own lists")
    func annotationKindsBlockTheirLists() {
        #expect(OutboxScope.isBlocked(CacheKey.highlights(uuid), by: [OpKind.highlight]))
        #expect(!OutboxScope.isBlocked(CacheKey.bookmarks(uuid), by: [OpKind.highlight]))
        #expect(OutboxScope.isBlocked(CacheKey.bookmarks(uuid), by: [OpKind.bookmark]))
        // Journals count toward the completion side of the stats summary.
        #expect(OutboxScope.isBlocked(CacheKey.journals(uuid), by: [OpKind.journal]))
        #expect(OutboxScope.isBlocked(CacheKey.stats(.year), by: [OpKind.journal]))
    }

    @Test("an empty queue blocks nothing")
    func emptyQueueBlocksNothing() {
        #expect(!OutboxScope.isBlocked(CacheKey.shelves, by: []))
        #expect(!OutboxScope.isBlocked(CacheKey.progress(uuid, .epub), by: []))
    }

    @Test("an undeclared kind is treated as blocking everything")
    func unknownKindIsConservative() {
        // A new write path that forgets to declare its scope should be
        // over-cautious rather than let a stale answer overwrite what it queued.
        #expect(OutboxScope.isBlocked(CacheKey.shelves, by: ["something_new"]))
        #expect(OutboxScope.isBlocked(CacheKey.me, by: ["something_new"]))
    }
}

// MARK: - Reachability signals

@Suite("Cancellation is not a reachability signal")
struct CancellationTests {
    @Test("a cancelled URL task is recognised as cancellation")
    func urlCancellationIsRecognised() {
        #expect(isCancellation(URLError(.cancelled)))
        #expect(isCancellation(CancellationError()))
    }

    @Test("a real transport failure is not cancellation")
    func transportFailuresAreNotCancellation() {
        // These are the ones that legitimately mark the server unreachable; a
        // scrolled-away cover fetch must not join them.
        #expect(!isCancellation(URLError(.notConnectedToInternet)))
        #expect(!isCancellation(URLError(.timedOut)))
        #expect(!isCancellation(URLError(.cannotConnectToHost)))
        #expect(!isCancellation(APIError.offline))
        #expect(!isCancellation(APIError.http(status: 500, message: "")))
    }
}

// MARK: - Replay outcome classification

@Suite("Replay outcomes")
struct ReplayOutcomeTests {
    @Test("a payload the server refuses on its merits is never retried")
    func refusalsAreTerminal() {
        for status in [400, 401, 403, 404, 409, 413, 422] {
            #expect(SyncEngine.isTerminalRejection(status), "\(status) should be terminal")
        }
    }

    @Test("a timeout or a throttle is about timing, not the payload, so it retries")
    func timingFailuresAreRetryable() {
        // A self-hosted server behind a proxy that rate-limits answers 429 to a
        // burst of queued writes. Retiring those as "rejected" discarded
        // reading positions and highlights that would have landed a minute
        // later.
        for status in [408, 425, 429] {
            #expect(!SyncEngine.isTerminalRejection(status), "\(status) should retry")
        }
    }

    @Test("server faults are retried rather than held aside")
    func serverFaultsAreRetryable() {
        for status in [500, 502, 503, 504] {
            #expect(!SyncEngine.isTerminalRejection(status))
        }
    }

    @Test("a success is not a rejection")
    func successesAreNotRejections() {
        for status in [200, 201, 204] {
            #expect(!SyncEngine.isTerminalRejection(status))
        }
    }
}

// MARK: - Position wire contract

@Suite("Progress client clock")
struct ProgressClockTests {
    private func encoded(_ update: ProgressUpdate) throws -> [String: Any] {
        let data = try JSONEncoder().encode(update)
        return try #require(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
    }

    @Test("a position carries the device's clock so a replay can be ordered")
    func clientClockIsOnTheWire() throws {
        // The whole conflict fix rests on this field reaching the server: a
        // position queued offline is replayed hours later, and without a clock
        // the server can only order by arrival — which makes the stale one win.
        let update = ProgressUpdate(
            bookUUID: "book-1", format: .epub,
            epubCFI: "epubcfi(/6/4)", audioPositionSeconds: nil,
            clientUpdatedAt: 1_700_000_000
        )
        let object = try encoded(update)
        #expect(object["client_updated_at"] as? Int64 == 1_700_000_000)
    }

    @Test("the clock defaults to now rather than being left off")
    func clientClockDefaultsToNow() throws {
        // Every call site builds one of these positionally; a default that had
        // to be passed explicitly would eventually be forgotten at one of them,
        // and that one would silently go back to last-write-wins.
        let before = Int64(Date().timeIntervalSince1970)
        let update = ProgressUpdate(
            bookUUID: "book-1", format: .audio,
            epubCFI: nil, audioPositionSeconds: 42
        )
        let stamped = try #require(try encoded(update)["client_updated_at"] as? Int64)
        #expect(stamped >= before)
    }

    @Test("two positions are ordered on the reader's clock, not the server's")
    func recordsOrderOnTheClientClock() {
        // The comparison that decides whether to offer a sync jump. `updatedAt`
        // cannot carry it: an optimistic row holds a device clock and a server
        // row holds the server's arrival clock, so a phone a few minutes ahead
        // of a self-hosted box suppressed every offer and one behind raised
        // them constantly.
        let local = ProgressRecord(
            bookUUID: "book-1", format: .epub, epubCFI: "epubcfi(/6/4)",
            audioPositionSeconds: nil, updatedAt: 9_000, clientUpdatedAt: 1_000
        )
        let remote = ProgressRecord(
            bookUUID: "book-1", format: .epub, epubCFI: "epubcfi(/6/9)",
            audioPositionSeconds: nil, updatedAt: 5_000, clientUpdatedAt: 2_000
        )
        #expect(remote.orderingClock > local.orderingClock)
        // Arrival order says the opposite, which is the bug.
        #expect(remote.updatedAt < local.updatedAt)
    }

    @Test("a record from a server too old to send the clock still orders")
    func orderingFallsBackToTheArrivalClock() {
        // `client_updated_at` arrived with migration 0050; against anything
        // older the field is absent and `updatedAt` is the only clock there is.
        let old = ProgressRecord(
            bookUUID: "book-1", format: .epub, epubCFI: "epubcfi(/6/4)",
            audioPositionSeconds: nil, updatedAt: 7_000, clientUpdatedAt: nil
        )
        #expect(old.orderingClock == 7_000)
    }

    @Test("a record decodes from a payload with no client clock")
    func recordDecodesWithoutTheClientClock() throws {
        let json = """
            {"book_uuid":"b","format":"epub","epub_cfi":"epubcfi(/6/4)",
             "audio_position_seconds":null,"updated_at":7000}
            """
        let record = try JSONDecoder().decode(ProgressRecord.self, from: Data(json.utf8))
        #expect(record.clientUpdatedAt == nil)
        #expect(record.orderingClock == 7_000)
    }
}

// MARK: - Audiobook file selection

@Suite("Progress file identity")
struct ProgressFileIdentityTests {
    private func encoded(_ update: ProgressUpdate) throws -> [String: Any] {
        let data = try JSONEncoder().encode(update)
        return try #require(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
    }

    @Test("a position carries the file it was taken in")
    func fileIdIsOnTheWire() throws {
        // Two narrations of one book don't share a timeline; without the file
        // id every resume lands the saved timestamp in the book's first file.
        let update = ProgressUpdate(
            bookUUID: "book-1", format: .audio,
            epubCFI: nil, audioPositionSeconds: 42, bookFileID: 917
        )
        let object = try encoded(update)
        #expect(object["book_file_id"] as? Int64 == 917)
    }

    @Test("an unknown file encodes as an absent field, not null")
    func unknownFileIsOmitted() throws {
        // A server that predates migration 0056 must see the payload it always
        // has — the web client omits the field the same way.
        let update = ProgressUpdate(
            bookUUID: "book-1", format: .audio,
            epubCFI: nil, audioPositionSeconds: 42
        )
        let object = try encoded(update)
        #expect(object["book_file_id"] == nil)
    }

    @Test("a record decodes the file id, and its absence")
    func recordDecodesTheFileId() throws {
        let with = """
            {"book_uuid":"b","format":"audio","audio_position_seconds":42,
             "updated_at":7000,"book_file_id":917}
            """
        let without = """
            {"book_uuid":"b","format":"audio","audio_position_seconds":42,
             "updated_at":7000}
            """
        let recorded = try JSONDecoder().decode(ProgressRecord.self, from: Data(with.utf8))
        let legacy = try JSONDecoder().decode(ProgressRecord.self, from: Data(without.utf8))
        #expect(recorded.bookFileID == 917)
        #expect(legacy.bookFileID == nil)
    }
}

@Suite("Selectable audio files")
struct AudioFileSelectionTests {
    private func file(
        _ id: Int64, format: String, ordinal: Int64
    ) -> BookFileInfo {
        BookFileInfo(
            id: id, format: format, filename: "file-\(id).\(format.lowercased())",
            ordinal: ordinal
        )
    }

    @Test("only formats the manifest resolver admits are offered")
    func onlyStreamableFormatsAreOffered() {
        // `resolve_audiobook_file` filters on M4B / M4A / MP3 — offering a
        // FLAC (or the epub of a dual-format book) as a choice would 404 the
        // moment it was picked.
        var book = Book(id: 1, filename: "b.epub")
        book.bookFiles = [
            file(1, format: "EPUB", ordinal: 0),
            file(2, format: "M4B", ordinal: 0),
            file(3, format: "FLAC", ordinal: 1),
            file(4, format: "MP3", ordinal: 2),
        ]
        #expect(book.audioFiles.map(\.id) == [2, 4])
    }

    @Test("files come back in the order the server resolves them")
    func filesAreOrderedByOrdinal() {
        // The first of these is the server's default (no `file_id`), which is
        // what makes "is the downloaded copy usable for this selection"
        // answerable at all.
        var book = Book(id: 1, filename: "b.m4b")
        book.bookFiles = [
            file(9, format: "M4B", ordinal: 2),
            file(7, format: "M4B", ordinal: 0),
            file(8, format: "M4A", ordinal: 1),
        ]
        #expect(book.audioFiles.map(\.id) == [7, 8, 9])
    }

    @Test("a manifest is cached per file, and the default key is unchanged")
    func manifestKeysAreFileScoped() {
        // One narration's timeline must never answer for another's — and the
        // no-file key has to keep its old shape so manifests cached before
        // selection existed (the download prefetch path) stay valid.
        #expect(CacheKey.manifest("u") == "manifest:u")
        #expect(CacheKey.manifest("u", fileID: nil) == "manifest:u")
        #expect(CacheKey.manifest("u", fileID: 5) == "manifest:u:5")
        #expect(CacheKey.manifest("u", fileID: 5) != CacheKey.manifest("u", fileID: 6))
    }
}

// MARK: - Account scoping

@Suite("User-scoped cache wipe")
struct UserScopedPrefixTests {
    private let uuid = "11111111-2222-3333-4444-555555555555"

    /// Every key `CacheKey` can mint that belongs to one account. A key added
    /// here without a matching prefix is a key the next user inherits.
    private var userScopedKeys: [String] {
        [
            CacheKey.me,
            CacheKey.recentProgress,
            CacheKey.shelves,
            CacheKey.shelfPreviews,
            CacheKey.shelf(7),
            CacheKey.shelfPage(7),
            CacheKey.shelvesContaining(uuid),
            CacheKey.progress(uuid, .epub),
            CacheKey.progress(uuid, .audio),
            CacheKey.highlights(uuid),
            CacheKey.bookmarks(uuid),
            CacheKey.journals(uuid),
            CacheKey.rating(uuid),
            CacheKey.ratingsOthers(uuid),
            CacheKey.readStatus(uuid),
            CacheKey.playbackRate(uuid),
            CacheKey.stats(.week),
        ]
    }

    @Test("every user-scoped cache key is covered by a wipe prefix")
    func userScopedKeysAreCovered() {
        // `shelf_previews` was not, for exactly the reason this test exists:
        // the list is matched by prefix and reads as though "shelves" covers
        // everything shelf-shaped. It doesn't — so after a second account
        // signed in on the same device, the Library landing rail went on
        // showing the first user's shelf names and cover artwork.
        for key in userScopedKeys {
            let covered = OfflineStore.userScopedPrefixes.contains { key.hasPrefix($0) }
            #expect(covered, "no wipe prefix covers the user-scoped key \"\(key)\"")
        }
    }

    @Test("library-wide keys survive an account switch")
    func libraryWideKeysAreNotWiped() {
        // These describe the server's library rather than anyone's account, so
        // wiping them would re-download the whole mirror on every sign-in.
        for key in [
            CacheKey.library, CacheKey.authors, CacheKey.series, CacheKey.tags,
            CacheKey.book(uuid), CacheKey.manifest(uuid), CacheKey.manifest(uuid, fileID: 9),
            CacheKey.suggestions(uuid), CacheKey.libraryPage("title.asc."),
        ] {
            let covered = OfflineStore.userScopedPrefixes.contains { key.hasPrefix($0) }
            #expect(!covered, "library-wide key \"\(key)\" is being wiped on account switch")
        }
    }
}
