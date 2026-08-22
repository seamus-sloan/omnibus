//  DownloadManager.swift
//  Offline file downloads for ebooks and audiobooks.
//
//  Port of `frontend/src/offline/downloads/`. The Rust side pairs the registry
//  with a loopback HTTP server so the WebView can play a local file; on iOS
//  `AVPlayer` and `WKWebView` both load `file://` URLs directly, so the
//  loopback hop is dropped entirely.

import Foundation
import Observation

/// Why a download could not even be planned — a failure that happens before
/// any bytes move, so nothing on the device is at risk.
enum DownloadPlanError: LocalizedError {
    /// The book plays only through the server's on-the-fly transcode, so its
    /// source files are not something this device could decode anyway.
    case unsupportedAudioFormat
    case notConfigured

    var errorDescription: String? {
        switch self {
        case .unsupportedAudioFormat:
            "This audiobook has to be converted by the server as it plays, so it can't be stored on this device."
        case .notConfigured:
            "No server configured."
        }
    }
}

@Observable
@MainActor
final class DownloadManager: NSObject {
    static let shared = DownloadManager()

    /// Must be stable across launches — iOS reattaches a background session by
    /// this identifier when it relaunches the app to deliver completions.
    static let sessionID = "app.omnibus.downloads"

    /// Registry mirror, keyed by `DownloadRecord.id` (`uuid:kind`), for
    /// synchronous view reads.
    private(set) var records: [String: DownloadRecord] = [:]

    /// Held by the app delegate while iOS is waiting to be told the app has
    /// finished handling a background session's completions.
    var backgroundCompletion: (() -> Void)?

    /// The completed record a re-download is standing in for, keyed by
    /// `DownloadRecord.id`. Held only while a replacement is in flight; the
    /// delegate puts it back if the replacement never lands.
    private var replacing: [String: DownloadRecord] = [:]

    /// Records whose download has been given up on — a failed part, a cancel.
    ///
    /// A multi-part book has several transfers in flight at once, so one of
    /// them failing leaves siblings that will go on landing afterwards. Left
    /// unmarked they would resurrect a row the failure had already settled,
    /// and write a second failure over the copy a `restoreReplaced` had just
    /// put back. Cleared when the record is started again.
    private var abandoned: Set<String> = []

    /// Records currently being moved into place. The last two parts of a book
    /// can land close enough together that both completions see every file
    /// done, and the second one to reach [`install`] would find the incoming
    /// files the first had already consumed and report the record failed on
    /// the strength of it.
    private var installing: Set<String> = []

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
    ///
    /// Merged onto what this process already holds rather than replacing it.
    /// Every step here suspends, and the background session delivers its
    /// completions on the main actor in exactly those gaps — a wholesale
    /// `records = map` would put back the snapshot read before them and lose
    /// the per-part `done` flags they had just set. A single lost flag is not
    /// cosmetic: `install` fires only when every file reports done, so the
    /// download would wedge at 99% forever and the next launch would sweep its
    /// staged parts away as stranded.
    func hydrate() async {
        let all = await OfflineStore.shared.allDownloads()
        let live = Set(
            await session.allTasks
                .compactMap(\.taskDescription)
                .map(DownloadRecord.recordID(fromTaskKey:))
        )

        for var record in all {
            // Already tracked here means this process has touched it since the
            // read above, and memory is written before the store is mirrored —
            // so the in-memory copy is the newer of the two by construction.
            if records[record.id] != nil { continue }
            if record.state == .running || record.state == .queued, !live.contains(record.id) {
                record.state = .failed
                record.error = "Interrupted"
                // Nothing resumes a stranded transfer, so the bytes it staged
                // are dead weight — invisible to "Storage used", which counts
                // completed records, and never reclaimed unless the reader
                // happens to retry.
                Self.discardIncoming(of: record)
                for index in record.files.indices { record.files[index].done = false }
                await OfflineStore.shared.upsertDownload(record)
            }
            records[record.id] = record
        }
        await healPartialAudiobooks()
    }

    /// Retire a completed audiobook record that only ever held part 1.
    ///
    /// The build this replaces stored one part of a multi-part audiobook and
    /// called the download complete. Those records survive the upgrade and go
    /// on claiming the book is on the device, while the player — which now
    /// checks that a local copy covers every part — refuses to use them. Left
    /// alone that is a book the shelf says is downloaded, the offline gate
    /// lets you open, and the player then cannot play. Marking it failed puts
    /// a Retry in front of the reader instead, which is the only thing that
    /// actually gets them the rest of the book.
    ///
    /// Only decided against a manifest the device already holds; a book whose
    /// manifest was never cached is left exactly as it is rather than guessed
    /// at.
    private func healPartialAudiobooks() async {
        for record in records.values
        where record.kind == .audio && record.state == .complete && record.isLegacyShape {
            guard let manifest: AudiobookManifest = await Cache.cachedOnly(
                CacheKey.manifest(record.bookUUID)
            ) else { continue }
            guard !AudioPlayer.localCovers(
                manifest: manifest, localFileCount: record.files.count
            ) else { continue }
            await update(key: record.id) { record in
                record.state = .failed
                record.error = "Only part of this audiobook was downloaded — download it again."
            }
        }
    }

    /// Drop everything this process is tracking, for a wipe that has already
    /// emptied the store — `hydrate` merges rather than replaces, so it can no
    /// longer be the thing that clears the registry.
    func forgetAll() {
        records = [:]
        replacing = [:]
        abandoned = []
        installing = []
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
        !localFiles(for: uuid, kind: kind).isEmpty
    }

    /// Whether any format of this book is on the device — what a shelf badge
    /// means, as opposed to what the player needs to know.
    ///
    /// Answered from the registry alone, with no filesystem check. This runs
    /// in the badge body of every visible grid tile, re-evaluated on every
    /// `records` mutation — which during a download is several times a second
    /// — so stat-ing each file of each format here would put N syscalls per
    /// tile per frame on the main actor. A badge that is briefly optimistic
    /// about a file deleted underneath us is a fair trade; the paths where
    /// being wrong actually costs something ([`isDownloaded`],
    /// [`localFiles`]) still check.
    func isAnyDownloaded(_ uuid: String) -> Bool {
        DownloadKind.allCases.contains { record(for: uuid, kind: $0)?.state == .complete }
    }

    /// Every local file of one format, in play order — one for an ebook or a
    /// single-file audiobook, one per part for a multi-part audiobook.
    ///
    /// All or nothing on purpose: a book missing one of its parts is not
    /// playable offline, and handing the player what is there would put it on
    /// a timeline shorter than the book, which is the whole failure this
    /// list exists to prevent.
    ///
    /// `kind` is not optional on purpose. Resolving by book alone is what let a
    /// dual-format book's downloaded epub be handed to `AVPlayer` as though it
    /// were the audiobook.
    func localFiles(for uuid: String, kind: DownloadKind) -> [URL] {
        guard let record = record(for: uuid, kind: kind), record.state == .complete,
              !record.files.isEmpty
        else { return [] }
        let urls = record.files
            .sorted { $0.ordinal < $1.ordinal }
            .map { OfflineStore.downloadsDirectory.appendingPathComponent($0.name) }
        guard urls.allSatisfy({ FileManager.default.fileExists(atPath: $0.path) }) else { return [] }
        return urls
    }

    /// The single local file for one format, or `nil` to stream.
    ///
    /// `nil` for a multi-part audiobook too — a caller holding one URL cannot
    /// represent a book that is several files, and treating part 1 as the
    /// whole book is what marked multi-part audiobooks finished the moment
    /// they were resumed past it. Part-aware callers use [`localFiles`].
    func localURL(for uuid: String, kind: DownloadKind) -> URL? {
        let files = localFiles(for: uuid, kind: kind)
        return files.count == 1 ? files[0] : nil
    }

    /// Which formats of this book can be taken offline. A dual-format book
    /// offers both — they are two files and a reader may want either.
    nonisolated static func kinds(for book: Book) -> [DownloadKind] {
        var kinds: [DownloadKind] = []
        if book.hasEbook { kinds.append(.ebook) }
        if book.hasAudiobook { kinds.append(.audio) }
        return kinds
    }

    /// The `book_files` row a download of `kind` targets: the lowest-ordinal
    /// file of a format the download endpoint actually **serves**, which is
    /// the same row the server resolves (`db::book_file_path`'s
    /// `ORDER BY bf.ordinal LIMIT 1`).
    ///
    /// The narrow format sets matter. `Book.ebookFormats` and
    /// `Book.audioFormats` describe what a library can *contain*; the
    /// endpoints serve EPUB (else CBZ for a comic-only book), and
    /// M4B/M4A/MP3. Matching on the broad sets lets a mixed book — a PDF at
    /// ordinal 0, the EPUB at ordinal 1 — snapshot the PDF's validator while
    /// downloading the EPUB, which then reports a stale download whenever
    /// the PDF changes and misses every change to the file actually on the
    /// device.
    nonisolated static func targetFile(_ book: Book, kind: DownloadKind) -> BookFileInfo? {
        if kind == .audio {
            return book.bookFiles
                .filter { Book.selectableAudioFormats.contains($0.format.lowercased()) }
                .min { $0.ordinal < $1.ordinal }
        }
        // Mirror `/file`'s two-step resolution exactly: the EPUB wins when
        // the book has one, and the CBZ answers only after that — not the
        // lowest ordinal across both, which would snapshot the CBZ's
        // validator on a dual-format book whose EPUB is what downloads.
        func lowest(_ format: String) -> BookFileInfo? {
            book.bookFiles
                .filter { $0.format.lowercased() == format }
                .min { $0.ordinal < $1.ordinal }
        }
        return lowest("epub") ?? lowest("cbz")
    }

    /// Whether the library file has moved under a downloaded copy — the
    /// "Update available" condition.
    ///
    /// `false` for a download that never finished — there is no local copy
    /// for a newer file to be newer *than*, which is an answer rather than a
    /// guess. `nil` only when the question genuinely cannot be answered: no
    /// download at all, a record predating validators, or metadata carrying
    /// no file rows (the library listing projection doesn't).
    /// Deliberately three-valued — a caller that *renders* may collapse
    /// "don't know" to false, but a caller that *stores* the answer must
    /// not, or it would clear a flag a real comparison had set.
    func staleness(of uuid: String, kind: DownloadKind, against book: Book) -> Bool? {
        guard let record = record(for: uuid, kind: kind) else { return nil }
        guard record.state == .complete else { return false }
        return Self.staleness(snapshot: record.sourceEtag, against: book, kind: kind)
    }

    /// The comparison itself, free of the registry so it can be exercised
    /// directly. `nil` whenever a validator is missing on either side — an
    /// older record, a file the scanner hasn't stat'd, or metadata with no
    /// file rows at all (the library listing projection has none).
    nonisolated static func staleness(
        snapshot: String?, against book: Book, kind: DownloadKind
    ) -> Bool? {
        guard let snapshot, let current = targetFile(book, kind: kind)?.etag else { return nil }
        return snapshot != current
    }

    /// Renderer-facing form of [`staleness(of:kind:against:)`]: "don't know"
    /// reads as "not stale", because prompting a reader to re-download on a
    /// guess is worse than staying quiet.
    func isStale(_ uuid: String, kind: DownloadKind, against book: Book) -> Bool {
        staleness(of: uuid, kind: kind, against: book) ?? false
    }

    /// Whether any downloaded format of this book has been superseded.
    func isAnyStale(_ book: Book) -> Bool {
        DownloadKind.allCases.contains { isStale(book.uuid, kind: $0, against: book) }
    }

    /// Total bytes held on disk by completed downloads — shown on the You tab.
    func totalBytesOnDisk() -> Int64 {
        records.values.filter { $0.state == .complete }.reduce(0) { $0 + $1.totalBytes }
    }

    // MARK: - Planning

    /// Extension for a manifest part's mime type — the inverse of the
    /// server's `mime_for_filename`, and a copy of the web engine's
    /// `mime_ext`. Only used to name the file on disk.
    nonisolated static func mimeExtension(_ mime: String) -> String {
        switch mime.lowercased() {
        case "audio/mpeg", "audio/mp3": "mp3"
        case "audio/x-m4a", "audio/m4a": "m4a"
        case "audio/aac": "aac"
        default: "m4b"
        }
    }

    /// The files a download of `kind` is made of, in play order.
    ///
    /// An audiobook is planned from its **manifest**, never from the download
    /// URL alone. `/api/audiobooks/{uuid}/download` serves one part per
    /// request, and the manifest is the only thing that says how many parts
    /// there are — so a plan derived from the URL called a four-part book
    /// complete after one part, and left the player treating part 1 as the
    /// whole book.
    nonisolated static func plan(
        book: Book, kind: DownloadKind, manifest: AudiobookManifest?
    ) throws -> [DownloadFile] {
        let uuid = book.uuid
        guard kind == .audio else {
            let format = targetFile(book, kind: .ebook)?.format.lowercased()
                ?? (book.opensAsComic ? "cbz" : "epub")
            return [
                DownloadFile(
                    ordinal: 0,
                    urlPath: "/api/ebooks/\(uuid)/file",
                    name: "\(uuid).\(DownloadKind.ebook.rawValue).\(format)"
                )
            ]
        }
        // An HLS-mode book is one the server has to transcode as it plays, so
        // its source files are not something this device can decode on its
        // own — there is nothing worth storing. The web engine refuses the
        // same books with `UnsupportedFormat`.
        guard case let .direct(parts, _, _) = manifest, !parts.isEmpty else {
            throw DownloadPlanError.unsupportedAudioFormat
        }
        let sorted = parts.sorted { $0.ordinal < $1.ordinal }
        return sorted.map { part in
            let ext = mimeExtension(part.mime)
            let stem = "\(uuid).\(DownloadKind.audio.rawValue)"
            return DownloadFile(
                ordinal: part.ordinal,
                // The gated `/download` route, not the part URL the manifest
                // hands out: `/parts/{ordinal}` is the playback stream and is
                // deliberately open to a reader whose account may listen but
                // may not keep a copy. Planning against it would hand that
                // reader the whole book.
                urlPath: sorted.count == 1
                    ? "/api/audiobooks/\(uuid)/download"
                    : "/api/audiobooks/\(uuid)/download?part=\(part.ordinal)",
                // A single-part book keeps the pre-multi-part name, so a copy
                // already on the device is still the file this plan names.
                name: sorted.count == 1
                    ? "\(stem).\(ext)"
                    : "\(stem).\(part.ordinal).\(ext)"
            )
        }
    }

    // MARK: - Writes

    /// Replace a completed download with the library's current bytes.
    ///
    /// **Not** `remove` followed by `start`. That deletes the only copy on
    /// the device before knowing a replacement will arrive, so any failure
    /// afterwards — unconfigured server, expired auth, no connectivity, a
    /// full disk, a dropped transfer — leaves the reader with nothing where
    /// a perfectly readable book used to be. A stale book beats no book.
    ///
    /// The existing files stay exactly where they are. Each transfer lands
    /// under its `incomingName` and the whole set is moved into place only
    /// once every file is down; if anything fails first, the previous record
    /// is restored and the files it points at were never touched.
    func redownload(book: Book, kind: DownloadKind) async {
        guard let previous = record(for: book.uuid, kind: kind), previous.state == .complete
        else {
            await start(book: book, kind: kind)
            return
        }
        await start(book: book, kind: kind, replacing: previous)
    }

    /// Begin (or restart) a download. `kind` picks the endpoint: an ebook pulls
    /// `/api/ebooks/{uuid}/file`, an audiobook one
    /// `/api/audiobooks/{uuid}/download` request per part.
    ///
    /// `replacing` carries the completed record this download is standing in
    /// for, so the book stays on disk throughout and is restored on failure.
    func start(book: Book, kind: DownloadKind, replacing previous: DownloadRecord? = nil) async {
        let uuid = book.uuid
        let key = DownloadRecord.key(uuid, kind)

        // Planned before anything is written, so a plan that can't be made
        // leaves the registry — and any copy already on the device —
        // untouched.
        let plan: [DownloadFile]
        do {
            plan = try Self.plan(
                book: book, kind: kind, manifest: try await Self.manifest(for: book, kind: kind)
            )
        } catch {
            // A replacement that never got started leaves the reader with the
            // book they already had, the same rule the delegate's failure
            // path follows — there is nothing to report over a file that is
            // still perfectly readable.
            guard previous == nil else { return }
            await fail(book: book, kind: kind, message: Self.message(for: error))
            return
        }

        var requests: [URLRequest] = []
        let headers = await APIClient.shared.authHeaders()
        for file in plan {
            guard let url = await APIClient.shared.absoluteURL(file.urlPath) else {
                guard previous == nil else { return }
                await fail(book: book, kind: kind, message: Self.message(for: DownloadPlanError.notConfigured))
                return
            }
            var request = URLRequest(url: url)
            for (header, value) in headers { request.setValue(value, forHTTPHeaderField: header) }
            requests.append(request)
        }

        // Anything still running for this key belongs to a previous attempt.
        // Left alone, its completions would land against the plan being
        // installed here and count files this attempt never fetched.
        for task in await session.allTasks
        where task.taskDescription.map(DownloadRecord.recordID(fromTaskKey:)) == key {
            task.cancel()
        }
        abandoned.remove(key)

        let record = DownloadRecord(
            bookUUID: uuid, kind: kind, format: Self.formatLabel(book, kind: kind, plan: plan),
            state: .running,
            files: plan,
            updatedAt: Int64(Date().timeIntervalSince1970),
            error: nil,
            // Snapshot what the library says this file is right now, so a
            // later refresh can tell us it has been replaced since. One value
            // for every part — they share a `book_files` row, so they share
            // its validator.
            sourceEtag: Self.targetFile(book, kind: kind)?.etag
        )
        if let previous { replacing[key] = previous }
        records[key] = record
        await OfflineStore.shared.upsertDownload(record)

        for (file, request) in zip(plan, requests) {
            let task = session.downloadTask(with: request)
            // The identity has to ride on the task itself: a background session
            // outlives the process, so an in-memory map from task identifier to
            // book is gone by the time the completion is delivered. iOS persists
            // `taskDescription` with the task.
            task.taskDescription = DownloadRecord.taskKey(key, ordinal: file.ordinal)
            task.resume()
        }

        // Everything else the book needs offline, pulled while the server is
        // still reachable. The transfers are the slow part and none of this
        // gates them, but all of it is unavailable the moment it's wanted: a
        // cover never scrolled past leaves a bare plate on the shelf, and
        // annotations never read leave a book that opens at the beginning with
        // nothing marked in it. The manifest an audiobook needs to be laid out
        // on a timeline is already cached — planning the parts fetched it.
        Task.detached(priority: .utility) {
            for size in [ThumbSize.sm, .md, .lg] {
                await ImageCache.shared.prefetch("/api/thumbs/\(uuid)/\(size.rawValue)")
            }
            await UserDataService.prefetchForOffline(uuid: uuid)
        }
    }

    /// The manifest a plan needs, or `nil` for an ebook.
    ///
    /// Fetched with no `file_id`, which is the file `/download` resolves on
    /// its own and the one `targetFile` snapshots the validator of — so the
    /// plan, the bytes, and the staleness check all describe the same file.
    /// It also warms the cache row the offline player reads.
    ///
    /// The error is propagated rather than swallowed. A manifest that could
    /// not be *fetched* and a book that genuinely cannot be stored are
    /// different answers, and collapsing them told a reader in a tunnel that
    /// a perfectly ordinary MP3 audiobook "has to be converted by the server"
    /// — a permanent-sounding verdict on a transient failure.
    private static func manifest(
        for book: Book, kind: DownloadKind
    ) async throws -> AudiobookManifest? {
        guard kind == .audio else { return nil }
        return try await LibraryService.audiobookManifest(uuid: book.uuid)
    }

    /// The badge a download row shows, and the extension a single-file
    /// download lands under.
    private static func formatLabel(_ book: Book, kind: DownloadKind, plan: [DownloadFile]) -> String {
        guard kind == .audio else {
            return targetFile(book, kind: .ebook)?.format.lowercased()
                ?? (book.opensAsComic ? "cbz" : "epub")
        }
        return (plan.first?.name as NSString?)?.pathExtension
            ?? book.formats.first { Book.audioFormats.contains($0.lowercased()) }
            ?? "m4b"
    }

    /// What the reader is told about a download that never started.
    ///
    /// A failure to reach the server says so in the terms the reader can act
    /// on. An audiobook has to be planned from its manifest before the first
    /// byte is asked for, so unlike an ebook it cannot be handed to the
    /// background session to complete whenever connectivity returns — the same
    /// trade the web engine makes with its known-offline fast fail.
    private static func message(for error: Error) -> String {
        if let api = error as? APIError, api.isRecoverableOffline {
            return "You're offline — connect to download."
        }
        return (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }

    /// Record a download that never started, so the reader sees why rather
    /// than a button that did nothing.
    private func fail(book: Book, kind: DownloadKind, message: String) async {
        let record = DownloadRecord(
            bookUUID: book.uuid, kind: kind, format: Self.formatLabel(book, kind: kind, plan: []),
            state: .failed, files: [],
            updatedAt: Int64(Date().timeIntervalSince1970),
            error: message,
            sourceEtag: nil
        )
        records[record.id] = record
        await OfflineStore.shared.upsertDownload(record)
    }

    func cancel(_ uuid: String, kind: DownloadKind) async {
        let key = DownloadRecord.key(uuid, kind)
        abandoned.insert(key)
        for task in await session.allTasks
        where task.taskDescription.map(DownloadRecord.recordID(fromTaskKey:)) == key {
            task.cancel()
        }
        await remove(uuid, kind: kind)
    }

    func remove(_ uuid: String, kind: DownloadKind) async {
        let key = DownloadRecord.key(uuid, kind)
        // Bytes fetched but not yet installed are this record's too, and the
        // store only knows about the files the record names.
        if let record = records[key] { Self.discardIncoming(of: record) }
        await OfflineStore.shared.deleteDownload(uuid, kind: kind)
        records[key] = nil
        replacing[key] = nil
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

    /// Put back the copy a failed replacement was standing in for, and
    /// report `true` when one was restored so the caller doesn't also write
    /// a failure over it.
    ///
    /// The files themselves were never touched — every transfer lands under
    /// an `incomingName` and the set is only installed on success — so
    /// restoring the record is enough to make the book readable again.
    private func restoreReplaced(key: String) async -> Bool {
        guard let previous = replacing.removeValue(forKey: key) else { return false }
        records[key] = previous
        await OfflineStore.shared.upsertDownload(previous)
        return true
    }

    /// A replacement landed, so the copy it stood in for is superseded.
    private func forgetReplaced(key: String) {
        replacing[key] = nil
    }

    private func update(key: String, mutate: (inout DownloadRecord) -> Void) async {
        guard var record = records[key] else { return }
        mutate(&record)
        record.updatedAt = Int64(Date().timeIntervalSince1970)
        records[key] = record
        await OfflineStore.shared.upsertDownload(record)
    }

    /// Adopt a record the delegate saw for a transfer this process didn't
    /// start — the relaunch case, where the registry is on disk but `records`
    /// has not been hydrated with it yet.
    private func adopt(key: String) async -> Bool {
        if records[key] != nil { return true }
        guard let (uuid, kind) = DownloadRecord.parse(key),
              let stored = await OfflineStore.shared.download(for: uuid, kind: kind)
        else { return false }
        records[key] = stored
        return true
    }

    // MARK: - Completion

    /// One file's bytes have landed: stage them under the file's incoming
    /// name, and install the whole record once nothing is left outstanding.
    fileprivate func finish(key: String, ordinal: Int64?, status: Int, staged: URL?) async {
        guard !abandoned.contains(key), await adopt(key: key) else {
            Self.discard(staged)
            return
        }
        // A completion for a record that is already installed is a duplicate
        // delivery, not new bytes — staging it would leave an incoming file
        // behind with nothing left to install it.
        guard records[key]?.state != .complete else {
            Self.discard(staged)
            return
        }
        guard (200..<300).contains(status), let staged,
              let record = records[key],
              let index = record.index(ofOrdinal: ordinal)
        else {
            Self.discard(staged)
            await abandon(key: key, message: "Download failed (\(status))")
            return
        }

        // CBZ integrity check *before* anything is installed, so a damaged
        // transfer never replaces a readable copy: every zip entry carries a
        // recorded CRC-32, and reading each to EOF verifies it — the
        // CRC-backed tier of rule 09's post-download backstop. Only comics get
        // this today; the EPUB/audio formats have no verifier on this client
        // yet.
        if record.format.lowercased() == "cbz" {
            let intact = await Task.detached(priority: .utility) {
                ComicArchive.verify(url: staged)
            }.value
            guard intact else {
                Self.discard(staged)
                await abandon(key: key, message: "The download failed its integrity check.")
                return
            }
        }

        let incoming = OfflineStore.downloadsDirectory
            .appendingPathComponent(record.files[index].incomingName)
        try? FileManager.default.removeItem(at: incoming)
        do {
            try FileManager.default.moveItem(at: staged, to: incoming)
        } catch {
            Self.discard(staged)
            await abandon(key: key, message: error.localizedDescription)
            return
        }

        let size = (try? FileManager.default
            .attributesOfItem(atPath: incoming.path)[.size] as? Int64) ?? 0
        await update(key: key) { record in
            guard let index = record.index(ofOrdinal: ordinal) else { return }
            record.files[index].done = true
            if size > 0 {
                record.files[index].receivedBytes = size
                if record.files[index].totalBytes == 0 { record.files[index].totalBytes = size }
            }
        }
        guard records[key]?.files.allSatisfy(\.done) == true else { return }
        await install(key: key)
    }

    /// Move every fetched file into place at once and mark the record
    /// complete.
    ///
    /// The all-at-once swap is what keeps a replacement honest: until this
    /// runs, the copy the reader already had is the only thing in the
    /// downloads directory under those names, so a failure at any point
    /// before it leaves them a whole book rather than a book with one part
    /// from each of two editions.
    private func install(key: String) async {
        guard !installing.contains(key), let record = records[key],
              record.state != .complete
        else { return }
        installing.insert(key)
        defer { installing.remove(key) }
        let fm = FileManager.default
        let directory = OfflineStore.downloadsDirectory
        let moves = record.files.map {
            (
                incoming: directory.appendingPathComponent($0.incomingName),
                destination: directory.appendingPathComponent($0.name)
            )
        }
        // Checked before the first destination is touched — a half-installed
        // set is the one outcome this ordering exists to rule out.
        guard moves.allSatisfy({ fm.fileExists(atPath: $0.incoming.path) }) else {
            await abandon(key: key, message: "The download is missing some of its files.")
            return
        }
        for move in moves {
            try? fm.removeItem(at: move.destination)
            do {
                try fm.moveItem(at: move.incoming, to: move.destination)
            } catch {
                await abandon(key: key, message: error.localizedDescription)
                return
            }
        }
        // Files the copy this one replaces held and this one doesn't — a book
        // that lost a part between downloads would otherwise leave it on disk
        // forever, counted under "Storage used" with no row pointing at it.
        if let previous = replacing[key] {
            let installed = Set(record.files.map(\.name))
            for file in previous.files where !installed.contains(file.name) {
                try? fm.removeItem(at: directory.appendingPathComponent(file.name))
            }
        }
        forgetReplaced(key: key)
        await update(key: key) { record in
            record.state = .complete
            record.error = nil
        }
    }

    /// Give up on a record: stop whatever else is still in flight for it,
    /// throw away the bytes staged so far, and either put back the copy this
    /// download was replacing or report the failure.
    fileprivate func abandon(key: String, message: String) async {
        guard !abandoned.contains(key) else { return }
        abandoned.insert(key)
        for task in await session.allTasks
        where task.taskDescription.map(DownloadRecord.recordID(fromTaskKey:)) == key {
            task.cancel()
        }
        if let record = records[key] { Self.discardIncoming(of: record) }
        if await restoreReplaced(key: key) { return }
        await update(key: key) { record in
            record.state = .failed
            record.error = message
            // Nothing here resumes, so the bytes are gone — saying otherwise
            // would leave a retry showing a progress bar starting part-full.
            for index in record.files.indices {
                record.files[index].done = false
                record.files[index].receivedBytes = 0
            }
        }
    }

    private static func discard(_ url: URL?) {
        guard let url else { return }
        try? FileManager.default.removeItem(at: url)
    }

    private static func discardIncoming(of record: DownloadRecord) {
        for file in record.files {
            try? FileManager.default.removeItem(
                at: OfflineStore.downloadsDirectory.appendingPathComponent(file.incomingName)
            )
        }
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
        guard let taskKey = downloadTask.taskDescription else { return }
        let key = DownloadRecord.recordID(fromTaskKey: taskKey)
        let ordinal = DownloadRecord.ordinal(fromTaskKey: taskKey)
        Task { @MainActor in
            guard await self.adopt(key: key) else { return }
            await self.update(key: key) { record in
                guard let index = record.index(ofOrdinal: ordinal) else { return }
                record.files[index].receivedBytes = totalBytesWritten
                if totalBytesExpectedToWrite > 0 {
                    record.files[index].totalBytes = totalBytesExpectedToWrite
                }
            }
        }
    }

    nonisolated func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didFinishDownloadingTo location: URL
    ) {
        guard let taskKey = downloadTask.taskDescription else { return }
        let key = DownloadRecord.recordID(fromTaskKey: taskKey)
        let ordinal = DownloadRecord.ordinal(fromTaskKey: taskKey)
        let status = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? 0
        // The temp file is deleted the moment this method returns, so the move
        // has to happen synchronously here, not in the Task below.
        let staged = OfflineStore.downloadsDirectory
            .appendingPathComponent("staged-\(UUID().uuidString)")
        let moved = (try? FileManager.default.moveItem(at: location, to: staged)) != nil

        Task { @MainActor in
            await self.finish(key: key, ordinal: ordinal, status: status, staged: moved ? staged : nil)
        }
    }

    nonisolated func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        guard let error, let taskKey = task.taskDescription else { return }
        // A cancel tears the record down on its own — and is also how a
        // failed sibling stops the rest of a multi-part book — so reporting it
        // would resurrect a row that has already been settled.
        guard (error as? URLError)?.code != .cancelled else { return }
        let key = DownloadRecord.recordID(fromTaskKey: taskKey)
        Task { @MainActor in
            guard await self.adopt(key: key) else { return }
            await self.abandon(key: key, message: error.localizedDescription)
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
