//  DownloadManager.swift
//  Offline file downloads for ebooks and audiobooks.
//
//  Port of `frontend/src/offline/downloads/`. The Rust side pairs the registry
//  with a loopback HTTP server so the WebView can play a local file; on iOS
//  `AVPlayer` and `WKWebView` both load `file://` URLs directly, so the
//  loopback hop is dropped entirely.

import Foundation
import Observation

@Observable
@MainActor
final class DownloadManager: NSObject {
    static let shared = DownloadManager()

    /// Must be stable across launches — iOS reattaches a background session by
    /// this identifier when it relaunches the app to deliver completions.
    static let sessionID = "app.omnibus.downloads"

    /// The server header naming the `book_files` row a download drew from.
    /// Kept in lockstep with `backend::validator::SOURCE_VALIDATOR`.
    static let sourceValidatorHeader = "X-Omnibus-Source-Validator"

    /// Registry mirror, keyed by `DownloadRecord.id` (`uuid:kind`), for
    /// synchronous view reads.
    private(set) var records: [String: DownloadRecord] = [:]

    /// Held by the app delegate while iOS is waiting to be told the app has
    /// finished handling a background session's completions.
    var backgroundCompletion: (() -> Void)?

    /// Completed records a re-download is superseding, keyed by record id.
    ///
    /// A replacement must not cost the reader the book they already have, so
    /// the old file is left in place — `didFinishDownloadingTo` only swaps it
    /// once the transfer has actually succeeded — and any failure before that
    /// puts this record back rather than leaving a `.failed` row over a file
    /// that is still perfectly readable.
    private var superseding: [String: DownloadRecord] = [:]

    private var session: URLSession!

    private override init() {
        super.init()
        // A background session, not a default one. A default session's
        // transfers are killed the moment the app is suspended, so locking the
        // phone part-way through an audiobook lost the whole download and left
        // a row that could only be restarted from zero. The system owns these
        // transfers instead, continues them while the app is suspended, and
        // relaunches the app to report them.
        let config = URLSessionConfiguration.background(withIdentifier: Self.sessionID)
        config.sessionSendsLaunchEvents = true
        // The reader asked for this one, so it should not wait for the system
        // to decide the moment is opportune.
        config.isDiscretionary = false
        config.timeoutIntervalForResource = 24 * 60 * 60
        session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }

    /// Reload the registry and reconcile it with what the system is still
    /// carrying.
    ///
    /// A transfer survives suspension and termination, so a `running` row is
    /// only stranded when nothing is behind it any more — checking that rather
    /// than assuming it is what keeps a download that is still going from being
    /// reported as interrupted the moment the app comes back.
    func hydrate() async {
        let all = await OfflineStore.shared.allDownloads()
        let live = Set(await session.allTasks.compactMap(\.taskDescription))

        var map: [String: DownloadRecord] = [:]
        for var record in all {
            if record.state == .running || record.state == .queued, !live.contains(record.id) {
                record.state = .failed
                record.error = "Interrupted"
                await OfflineStore.shared.upsertDownload(record)
            }
            // Records written before `localPath` became container-relative
            // hold a path under a stale container UUID. The file itself is
            // still there under its own name, so recover it rather than
            // making the reader re-download a book it already has.
            if let stored = record.localPath, stored.contains("/") {
                record.localPath = (stored as NSString).lastPathComponent
                await OfflineStore.shared.upsertDownload(record)
            }
            map[record.id] = record
        }
        records = map
    }

    // MARK: - Reads

    func record(for uuid: String, kind: DownloadKind) -> DownloadRecord? {
        records[DownloadRecord.key(uuid, kind)]
    }

    /// Every format of `uuid` held on this device.
    func records(for uuid: String) -> [DownloadRecord] {
        DownloadKind.allCases.compactMap { record(for: uuid, kind: $0) }
    }

    func isDownloaded(_ uuid: String, kind: DownloadKind) -> Bool {
        localURL(for: uuid, kind: kind) != nil
    }

    /// Whether any format of this book is on the device — what a shelf badge
    /// means, as opposed to what the player needs to know.
    func isAnyDownloaded(_ uuid: String) -> Bool {
        DownloadKind.allCases.contains { isDownloaded(uuid, kind: $0) }
    }

    /// The local file for one format, or `nil` to stream.
    ///
    /// `kind` is not optional on purpose. Resolving by book alone is what let a
    /// dual-format book's downloaded epub be handed to `AVPlayer` as though it
    /// were the audiobook.
    func localURL(for uuid: String, kind: DownloadKind) -> URL? {
        guard let record = record(for: uuid, kind: kind), record.state == .complete,
              let name = record.localPath
        else { return nil }
        let url = OfflineStore.downloadsDirectory.appendingPathComponent(name)
        return FileManager.default.fileExists(atPath: url.path) ? url : nil
    }

    /// Whether the completed download of `kind` is no longer the file the
    /// server holds — the book was repaired, re-uploaded, or replaced on disk
    /// since it was taken offline.
    ///
    /// `book` is whatever the caller last read; reads are network-first while
    /// online, so a replaced file surfaces on that pass. Answers `false` for a
    /// download still running, and for either side missing a validator — the
    /// honest reading of "nothing to compare" is "not known to be stale",
    /// since the alternative nags at every reader whose library predates the
    /// server recording one.
    func isStale(_ book: Book, kind: DownloadKind) -> Bool {
        guard let record = record(for: book.uuid, kind: kind), record.state == .complete,
              let takenUnder = record.validator,
              let current = book.sourceValidator(for: kind)
        else { return false }
        return current != takenUnder
    }

    /// Replace a superseded copy with the file the server holds now.
    ///
    /// Deliberately not `remove` then `start`. Removing first destroys the
    /// reader's only offline copy before anything is known about whether a
    /// replacement is obtainable, so a dropped connection, an expired session,
    /// a full disk, or a 500 would leave them with nothing. The download
    /// already lands in a temp file and is moved into place only on a 2xx, so
    /// leaving the old file alone makes the swap the first moment their copy
    /// changes at all.
    func redownload(_ book: Book, kind: DownloadKind) async {
        let key = DownloadRecord.key(book.uuid, kind)
        guard let prior = records[key], prior.state == .complete else { return }
        superseding[key] = prior
        await start(book: book, kind: kind)
    }

    /// Put back the copy a failed replacement was superseding, if any.
    /// `true` when it did, meaning the caller must not write a failure row.
    private func restoreSuperseded(key: String) async -> Bool {
        guard let prior = superseding.removeValue(forKey: key) else { return false }
        records[key] = prior
        await OfflineStore.shared.upsertDownload(prior)
        return true
    }

    /// Which formats of this book can be taken offline. A dual-format book
    /// offers both — they are two files and a reader may want either.
    nonisolated static func kinds(for book: Book) -> [DownloadKind] {
        var kinds: [DownloadKind] = []
        if book.hasEbook { kinds.append(.ebook) }
        if book.hasAudiobook { kinds.append(.audio) }
        return kinds
    }

    /// Total bytes held on disk by completed downloads — shown on the You tab.
    func totalBytesOnDisk() -> Int64 {
        records.values.filter { $0.state == .complete }.reduce(0) { $0 + $1.totalBytes }
    }

    // MARK: - Writes

    /// Begin (or restart) a download. `kind` picks the endpoint: an ebook pulls
    /// `/api/ebooks/{uuid}/file`, an audiobook `/api/audiobooks/{uuid}/download`.
    func start(book: Book, kind: DownloadKind) async {
        let uuid = book.uuid
        let path = kind == .audio
            ? "/api/audiobooks/\(uuid)/download"
            : "/api/ebooks/\(uuid)/file"
        let format = kind == .audio
            ? (book.formats.first { Book.audioFormats.contains($0.lowercased()) } ?? "m4b")
            : "epub"

        guard let url = await APIClient.shared.absoluteURL(path) else { return }
        var request = URLRequest(url: url)
        for (key, value) in await APIClient.shared.authHeaders() {
            request.setValue(value, forHTTPHeaderField: key)
        }

        // A replacement keeps the superseded file's name on the record: the
        // bytes are still on disk and still the reader's, right up until the
        // swap, and blanking it here would strand them behind a `nil`.
        let superseded = superseding[DownloadRecord.key(uuid, kind)]
        let record = DownloadRecord(
            bookUUID: uuid, kind: kind, format: format, state: .running,
            localPath: superseded?.localPath,
            totalBytes: 0, receivedBytes: 0, updatedAt: Int64(Date().timeIntervalSince1970),
            error: nil,
            // Snapshotted from the book being downloaded, so a later refresh
            // can tell this copy from the one the server holds.
            validator: book.sourceValidator(for: kind)
        )
        records[record.id] = record
        await OfflineStore.shared.upsertDownload(record)

        let task = session.downloadTask(with: request)
        // The identity has to ride on the task itself: a background session
        // outlives the process, so an in-memory map from task identifier to
        // book is gone by the time the completion is delivered. iOS persists
        // `taskDescription` with the task.
        task.taskDescription = record.id
        task.resume()

        // Everything else the book needs offline, pulled while the server is
        // still reachable. The transfer is the slow part and none of this
        // gates it, but all of it is unavailable the moment it's wanted: a
        // cover never scrolled past leaves a bare plate on the shelf, a
        // manifest never fetched leaves an audiobook that can't be laid out on
        // a timeline, and annotations never read leave a book that opens at
        // the beginning with nothing marked in it.
        Task.detached(priority: .utility) {
            for size in [ThumbSize.sm, .md, .lg] {
                await ImageCache.shared.prefetch("/api/thumbs/\(uuid)/\(size.rawValue)")
            }
            if kind == .audio {
                _ = try? await LibraryService.audiobookManifest(uuid: uuid)
            }
            await UserDataService.prefetchForOffline(uuid: uuid)
        }
    }

    func cancel(_ uuid: String, kind: DownloadKind) async {
        let key = DownloadRecord.key(uuid, kind)
        for task in await session.allTasks where task.taskDescription == key {
            task.cancel()
        }
        await remove(uuid, kind: kind)
    }

    func remove(_ uuid: String, kind: DownloadKind) async {
        await OfflineStore.shared.deleteDownload(uuid, kind: kind)
        records[DownloadRecord.key(uuid, kind)] = nil
    }

    /// Drop registry keys already removed from the store, so the in-memory
    /// mirror matches. For the library sync's orphan sweep, which deletes the
    /// rows and the files itself.
    func forget(_ keys: [String]) {
        for key in keys { records[key] = nil }
    }

    /// Drop every format of a book — what a swipe on the Downloads list means.
    func removeAll(_ uuid: String) async {
        for kind in DownloadKind.allCases {
            guard records[DownloadRecord.key(uuid, kind)] != nil else { continue }
            await remove(uuid, kind: kind)
        }
    }

    fileprivate func update(key: String, mutate: (inout DownloadRecord) -> Void) async {
        guard var record = records[key] else { return }
        mutate(&record)
        record.updatedAt = Int64(Date().timeIntervalSince1970)
        records[key] = record
        await OfflineStore.shared.upsertDownload(record)
    }

    /// Adopt a record the delegate saw for a transfer this process didn't
    /// start — the relaunch case, where the registry is on disk but `records`
    /// has not been hydrated with it yet.
    fileprivate func adopt(key: String) async -> Bool {
        if records[key] != nil { return true }
        guard let (uuid, kind) = DownloadRecord.parse(key),
              let stored = await OfflineStore.shared.download(for: uuid, kind: kind)
        else { return false }
        records[key] = stored
        return true
    }
}

extension DownloadManager: URLSessionDownloadDelegate {
    nonisolated func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64,
        totalBytesExpectedToWrite: Int64
    ) {
        guard let key = downloadTask.taskDescription else { return }
        Task { @MainActor in
            guard await self.adopt(key: key) else { return }
            await self.update(key: key) { record in
                record.receivedBytes = totalBytesWritten
                if totalBytesExpectedToWrite > 0 { record.totalBytes = totalBytesExpectedToWrite }
            }
        }
    }

    nonisolated func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didFinishDownloadingTo location: URL
    ) {
        guard let key = downloadTask.taskDescription else { return }
        let http = downloadTask.response as? HTTPURLResponse
        let status = http?.statusCode ?? 0
        // What the server says it served, which beats the metadata read from
        // before the transfer: a source replaced in between would otherwise
        // leave this copy recorded under a validator that never described it.
        // The `ETag` is not a substitute — on the audiobook route it names one
        // part, while the book's metadata reports the whole row.
        let servedUnder = http?.value(forHTTPHeaderField: Self.sourceValidatorHeader)
        // The temp file is deleted the moment this method returns, so the move
        // has to happen synchronously here, not in the Task below.
        let staged = OfflineStore.downloadsDirectory
            .appendingPathComponent("staged-\(UUID().uuidString)")
        let moved = (try? FileManager.default.moveItem(at: location, to: staged)) != nil

        Task { @MainActor in
            guard await self.adopt(key: key) else {
                try? FileManager.default.removeItem(at: staged)
                return
            }

            guard (200..<300).contains(status), moved else {
                try? FileManager.default.removeItem(at: staged)
                guard await self.restoreSuperseded(key: key) == false else { return }
                await self.update(key: key) { record in
                    record.state = .failed
                    record.error = "Download failed (\(status))"
                }
                return
            }

            guard let record = self.records[key] else { return }
            let name = "\(record.bookUUID).\(record.kind.rawValue).\(record.format)"
            let destination = OfflineStore.downloadsDirectory.appendingPathComponent(name)
            try? FileManager.default.removeItem(at: destination)
            do {
                try FileManager.default.moveItem(at: staged, to: destination)
            } catch {
                try? FileManager.default.removeItem(at: staged)
                guard await self.restoreSuperseded(key: key) == false else { return }
                await self.update(key: key) { record in
                    record.state = .failed
                    record.error = error.localizedDescription
                }
                return
            }

            let size = (try? FileManager.default
                .attributesOfItem(atPath: destination.path)[.size] as? Int64) ?? 0
            self.superseding[key] = nil
            await self.update(key: key) { record in
                record.state = .complete
                record.localPath = name
                if size > 0 {
                    record.receivedBytes = size
                    if record.totalBytes == 0 { record.totalBytes = size }
                }
                if let servedUnder { record.validator = servedUnder }
                record.error = nil
            }
        }
    }

    nonisolated func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        guard let error, let key = task.taskDescription else { return }
        Task { @MainActor in
            guard await self.adopt(key: key) else { return }
            // A cancel tears the record down on its own; reporting it as a
            // failure would resurrect the row the cancel just removed.
            guard (error as? URLError)?.code != .cancelled else {
                self.superseding[key] = nil
                return
            }
            guard await self.restoreSuperseded(key: key) == false else { return }
            await self.update(key: key) { record in
                record.state = .failed
                record.error = error.localizedDescription
            }
        }
    }

    /// Every completion from a relaunch has been delivered. iOS is holding the
    /// app awake for this call and suspends it again once the handler runs.
    nonisolated func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
        Task { @MainActor in
            let handler = self.backgroundCompletion
            self.backgroundCompletion = nil
            handler?()
        }
    }
}
