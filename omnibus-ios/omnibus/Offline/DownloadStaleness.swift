//  DownloadStaleness.swift
//  Learning that the library file under a downloaded copy has been replaced.
//
//  Port of `frontend/src/offline/downloads/staleness.rs`. `DownloadManager`
//  answers the question per book from metadata it already holds; this answers
//  it for the whole device at once, on a TTL, via one
//  `POST /api/downloads/validators` — so the Downloads screen learns of a
//  replaced file without the reader opening each book's detail page.

import Foundation

/// Which of a book's two downloadable formats a validator query is about.
///
/// Wire vocabulary of `omnibus_shared::DownloadFormat`, which spells the ebook
/// side `epub` where the registry's own `DownloadKind` spells it `ebook`.
enum DownloadValidatorFormat: String, Codable, Sendable {
    case epub, audio

    init(_ kind: DownloadKind) {
        self = kind == .audio ? .audio : .epub
    }

    var kind: DownloadKind { self == .audio ? .audio : .ebook }
}

/// One "what is this file's validator now?" question.
///
/// Carries no `file_id`: an iOS download never picks a `book_files` row, so
/// the row the server resolves by default is exactly the one it served.
struct DownloadValidatorQuery: Codable, Sendable, Equatable {
    var bookUUID: String
    var format: DownloadValidatorFormat

    enum CodingKeys: String, CodingKey {
        case format
        case bookUUID = "book_uuid"
    }
}

/// The answer to one query. A `nil` etag — the book, the format or the file is
/// gone, or the scanner has not stat'd it — means "can't tell", never
/// "unchanged".
struct DownloadValidator: Codable, Sendable, Equatable {
    var bookUUID: String
    var format: DownloadValidatorFormat
    var etag: String?

    enum CodingKeys: String, CodingKey {
        case format, etag
        case bookUUID = "book_uuid"
    }
}

struct DownloadValidatorRequest: Codable, Sendable {
    var files: [DownloadValidatorQuery]
}

struct DownloadValidatorResponse: Codable, Sendable {
    var files: [DownloadValidator]
}

extension DownloadManager {
    /// Most files one request may ask about — mirrors
    /// `omnibus_shared::MAX_VALIDATOR_QUERY`. `post_download_validators`
    /// answers `422` to anything larger, so a device holding more downloads
    /// than this asks in several requests rather than one rejected one.
    static let maxValidatorQuery = 500

    /// How long a swept answer stays good enough.
    ///
    /// A file changing on the server is not urgent, and the alternative —
    /// asking on every tick — is a data and battery cost that grows with the
    /// reader's library. Opening a book still compares that book immediately
    /// against the detail metadata it fetches anyway.
    static let staleCheckTTL: TimeInterval = 900

    /// Refresh the "Update available" flag for every completed download from
    /// the server's current validators.
    ///
    /// Driven by the moments a reader would notice the answer — coming back to
    /// the app, opening the Downloads screen — and paced by
    /// [`staleCheckTTL`] rather than run on each of them, so a device with
    /// fifty downloads makes one small request a quarter-hour instead of fifty
    /// full metadata fetches every time a screen appears.
    ///
    /// It writes **no cache rows**. The response carries validators and
    /// nothing else, so there is nothing to smuggle past `Cache`'s
    /// compare/put/notify path — which, bypassed, leaves an open screen
    /// rendering fields the cache has already replaced.
    func refreshStaleFlags() async {
        guard Connectivity.shared.isOnline else { return }
        let now = Date()
        if let last = lastValidatorSweep, now.timeIntervalSince(last) < Self.staleCheckTTL {
            return
        }

        // Nothing to ask about does not count as having asked. The registry
        // hydrates asynchronously, so an empty read at launch may only mean
        // "not loaded yet" — stamping it would skip the first real sweep for a
        // whole TTL. Re-reading an empty registry costs nothing.
        let queries = Self.validatorQueries(for: Array(records.values))
        guard !queries.isEmpty else { return }

        if await sweepValidators(queries) { lastValidatorSweep = now }
        // A sweep that failed part-way leaves the stamp alone, so the next
        // trigger retries the whole of it rather than waiting out a TTL on a
        // sweep that never finished.
    }

    /// Ask about `queries` in chunks the server will accept, applying each
    /// chunk's answers as they arrive rather than buffering them for one final
    /// apply — a later chunk failing still leaves the earlier ones' flags
    /// updated. Answers whether every chunk succeeded.
    func sweepValidators(_ queries: [DownloadValidatorQuery]) async -> Bool {
        for chunk in Self.chunked(queries, size: Self.maxValidatorQuery) {
            do {
                let answer: DownloadValidatorResponse = try await APIClient.shared.post(
                    "/api/downloads/validators",
                    body: DownloadValidatorRequest(files: chunk)
                )
                await applyValidators(answer.files)
            } catch {
                return false
            }
        }
        return true
    }

    /// One query per completed download, ordered by uuid so a request is
    /// reproducible in a log.
    ///
    /// Only completed downloads: an unfinished one has no local copy for a
    /// newer file to be newer *than*, and flagging it would invite a reader to
    /// replace bytes that are still arriving.
    nonisolated static func validatorQueries(
        for records: [DownloadRecord]
    ) -> [DownloadValidatorQuery] {
        records
            .filter { $0.state == .complete }
            .map { DownloadValidatorQuery(bookUUID: $0.bookUUID, format: .init($0.kind)) }
            .sorted { ($0.bookUUID, $0.format.rawValue) < ($1.bookUUID, $1.format.rawValue) }
    }

    /// Split a sweep into requests the server will accept. An empty list asks
    /// nothing rather than sending one empty request.
    nonisolated static func chunked(
        _ queries: [DownloadValidatorQuery], size: Int
    ) -> [[DownloadValidatorQuery]] {
        guard size > 0 else { return queries.isEmpty ? [] : [queries] }
        return stride(from: 0, to: queries.count, by: size).map {
            Array(queries[$0..<min($0 + size, queries.count)])
        }
    }

    /// Store what a batch of answers concluded, in memory and on disk.
    func applyValidators(_ answers: [DownloadValidator]) async {
        for record in answers.compactMap({ noteValidator($0) }) {
            await OfflineStore.shared.upsertDownload(record)
        }
    }

    /// The flag `record` should carry after `answer`, or `nil` when the answer
    /// changes nothing.
    ///
    /// `nil` covers three cases that must not be collapsed into "not stale".
    /// A record that never finished has no copy to supersede. A validator
    /// missing on either side — a record written before validators existed, a
    /// `book_files` row the scanner has not stat'd, a book the server no
    /// longer has — is a question that could not be answered, and this writes
    /// its result down, so treating it as "fresh" would clear a flag a real
    /// comparison had set. And an answer that agrees with the stored flag is
    /// not worth a database write.
    nonisolated static func staleUpdate(
        for record: DownloadRecord, from answer: DownloadValidator
    ) -> Bool? {
        guard record.state == .complete,
              let stale = staleness(snapshot: record.sourceEtag, current: answer.etag),
              stale != record.stale
        else { return nil }
        return stale
    }

    /// The comparison itself: the validator the download snapshotted against
    /// the one the library carries now.
    nonisolated static func staleness(snapshot: String?, current: String?) -> Bool? {
        guard let snapshot, let current else { return nil }
        return snapshot != current
    }
}
