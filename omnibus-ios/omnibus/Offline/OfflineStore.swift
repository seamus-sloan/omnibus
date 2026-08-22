//  OfflineStore.swift
//  Durable SQLite store backing the offline layer: replica cache, mutation
//  outbox, and download registry.
//
//  Port of `frontend/src/offline/store.rs`. Best-effort by design — if the
//  data dir or the SQLite open fails, every accessor degrades to a no-op and
//  the app behaves exactly like an online-only build.

import Foundation
import SQLite3

/// SQLite wants to know whether it may keep a pointer to the bound bytes.
private let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

/// One queued mutation awaiting replay against the server.
struct PendingOp: Sendable, Identifiable {
    var id: Int64
    var kind: String
    var path: String
    var method: String
    var body: Data?
    var createdAt: Int64
    var attempts: Int64
    /// Set when a replay failed with a non-retryable status, so the UI can
    /// surface it instead of retrying forever.
    var lastError: String?
}

/// Which of a book's two possible files a download holds.
///
/// A dual-format book is one book with two files, and a reader who takes it
/// offline wants the one they are about to use — so the registry is keyed on
/// the pair, not on the book. Keying on the book alone is what let a downloaded
/// epub be handed to `AVPlayer` as if it were the audiobook.
enum DownloadKind: String, Sendable, CaseIterable {
    case ebook, audio

    /// Container extensions that mean the file is an audiobook, used to place
    /// rows written before the registry knew about kinds.
    static func inferred(fromFormat format: String) -> DownloadKind {
        Book.audioFormats.contains(format.lowercased()) ? .audio : .ebook
    }
}

/// One file a download is made of. An ebook or a single-file audiobook has
/// exactly one; a multi-part audiobook has one per part, in ordinal order.
///
/// Mirrors `PlannedFile` in `frontend/src/offline/downloads.rs`, which has
/// planned per-part since the web client learned to store audiobooks.
struct DownloadFile: Codable, Sendable, Equatable {
    /// Audio part ordinal. `0` for an ebook, whose download is one file.
    var ordinal: Int64
    /// Server path this file is fetched from, query included.
    var urlPath: String
    /// File **name** inside `OfflineStore.downloadsDirectory`, never an
    /// absolute path.
    ///
    /// The container directory carries a UUID that iOS reassigns when the app
    /// is reinstalled, so an absolute path stored here goes stale and every
    /// downloaded book silently reverts to streaming — unreadable offline
    /// despite the file still sitting on disk. Resolve through `localURL`.
    var name: String
    /// Bytes on disk so far — the final size once `done`.
    var receivedBytes: Int64 = 0
    /// The size the server reported for this file; `0` until it has.
    var totalBytes: Int64 = 0
    var done: Bool = false

    /// Where this file's transfer lands while the download is still running.
    ///
    /// A download is installed all at once, at the very end. The parts of an
    /// audiobook arrive at different times, and moving each into place as it
    /// lands would assemble a half-replaced book — the new part 1 beside the
    /// old part 2 — over a copy the reader still has and can still play.
    var incomingName: String { "incoming.\(name)" }

    /// Where the copy this file supersedes is set aside for the length of an
    /// install, so a swap that fails half way through can put every file back
    /// exactly as it was.
    var supersededName: String { "superseded.\(name)" }

    /// Every name this file may occupy on disk. What a removal has to sweep:
    /// a crash mid-install can leave any of the three behind, and only the
    /// record knows they exist.
    var onDiskNames: [String] { [name, incomingName, supersededName] }

    /// How far along this one file is. Monotonic: it only ever rises, and a
    /// file whose size the server hasn't stated yet reads 0 rather than 1.
    var fraction: Double {
        if done { return 1 }
        guard totalBytes > 0 else { return 0 }
        return min(1, Double(receivedBytes) / Double(totalBytes))
    }
}

/// A book whose files have been pulled down for offline use.
struct DownloadRecord: Sendable, Identifiable {
    enum State: String, Sendable {
        case queued, running, complete, failed
    }

    var id: String { Self.key(bookUUID, kind) }

    /// Registry identity for a (book, format) pair. Also the stem of what
    /// rides on a download task's `taskDescription`, so a completion
    /// delivered after a relaunch can still say what it belongs to.
    static func key(_ uuid: String, _ kind: DownloadKind) -> String {
        "\(uuid):\(kind.rawValue)"
    }

    /// Split a key back apart. A book uuid contains no colon, so the last
    /// component is unambiguously the kind.
    static func parse(_ key: String) -> (uuid: String, kind: DownloadKind)? {
        guard let separator = key.lastIndex(of: ":"),
              let kind = DownloadKind(rawValue: String(key[key.index(after: separator)...]))
        else { return nil }
        return (String(key[key.startIndex..<separator]), kind)
    }

    /// What one file's transfer carries as its `taskDescription`: the record
    /// key plus the part it is fetching. A multi-part book hands several
    /// tasks to one background session at once, so the record key alone can
    /// no longer say which file a completion belongs to.
    static func taskKey(_ recordID: String, ordinal: Int64) -> String {
        "\(recordID)#\(ordinal)"
    }

    /// The record key a `taskDescription` belongs to. Tolerates the
    /// un-suffixed form so a transfer started by a build that predates
    /// per-part tasks is still attributed after an upgrade.
    static func recordID(fromTaskKey key: String) -> String {
        key.split(separator: "#", maxSplits: 1).first.map(String.init) ?? key
    }

    /// The part ordinal a `taskDescription` names, or `nil` for the
    /// un-suffixed legacy form.
    static func ordinal(fromTaskKey key: String) -> Int64? {
        let parts = key.split(separator: "#", maxSplits: 1)
        guard parts.count == 2 else { return nil }
        return Int64(parts[1])
    }

    var bookUUID: String
    var kind: DownloadKind
    var format: String
    var state: State
    /// Every file this download is made of, in play order.
    ///
    /// A list rather than one path because of multi-part audiobooks: the
    /// download route serves one part per request, so a record that could
    /// only name a single file left the player treating part 1 as the whole
    /// book — every resume past its end clamped backwards and marked the
    /// book finished.
    var files: [DownloadFile]
    var updatedAt: Int64
    var error: String?
    /// `BookFileInfo.etag` for the file this download came from, as it read
    /// when the download started. Compared against a later metadata refresh
    /// to drive "Update available"; `nil` for records written before
    /// validators existed, which reads as "can't tell", never as "stale".
    ///
    /// One value for the whole record, not one per file: every part of an
    /// audiobook belongs to the same `book_files` row and shares its
    /// validator.
    var sourceEtag: String?

    /// Where in [`files`] a transfer's part ordinal lands. A `nil` ordinal is
    /// a task started by a build that predates per-part downloads, which by
    /// construction had exactly one file.
    func index(ofOrdinal ordinal: Int64?) -> Int? {
        guard let ordinal else { return files.isEmpty ? nil : 0 }
        return files.firstIndex { $0.ordinal == ordinal }
    }

    /// The single file this download consists of, or `nil` when it has more
    /// than one.
    ///
    /// Also what gets mirrored to the legacy `local_path` column, so a build
    /// that predates multi-part downloads reads a multi-part record as
    /// having no local copy — and streams the book — rather than playing
    /// part 1 as though it were all of it.
    var localPath: String? { files.count == 1 ? files[0].name : nil }

    /// Whether this record's file list was reconstructed from the pre-list
    /// columns rather than stored as one — a download taken by a build that
    /// could only hold a single file per book. Those carry no URL, because
    /// the row never recorded one.
    var isLegacyShape: Bool { files.contains { $0.urlPath.isEmpty } }

    var receivedBytes: Int64 { files.reduce(0) { $0 + $1.receivedBytes } }

    var totalBytes: Int64 { files.reduce(0) { $0 + max($1.totalBytes, $1.receivedBytes) } }

    /// Progress across the whole download, as the mean of its files' own
    /// progress.
    ///
    /// Deliberately not `receivedBytes / totalBytes`: a part the system has
    /// not started yet contributes no size, so that denominator *grows* as
    /// parts begin and the bar walks up to 100% and falls back repeatedly
    /// through a multi-part book. The file count is known from the moment the
    /// download is planned, so averaging over it only ever rises. It weights
    /// a short part like a long one, which is the price of a bar that doesn't
    /// lie about direction.
    var fraction: Double {
        if state == .complete { return 1 }
        guard !files.isEmpty else { return 0 }
        return min(1, files.reduce(0.0) { $0 + $1.fraction } / Double(files.count))
    }
}

actor OfflineStore {
    static let shared = OfflineStore()

    private var db: OpaquePointer?
    private(set) var isOpen = false

    /// Files live beside the DB under Application Support, which iOS excludes
    /// from iCloud backup only if we ask — downloads are re-fetchable, so we
    /// do ask, and the cache DB is small enough to leave backed up.
    static var dataDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("Omnibus", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static var downloadsDirectory: URL {
        let dir = dataDirectory.appendingPathComponent("downloads", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var mutable = dir
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? mutable.setResourceValues(values)
        return dir
    }

    func open() {
        guard !isOpen else { return }
        let path = Self.dataDirectory.appendingPathComponent("offline.sqlite").path
        guard sqlite3_open(path, &db) == SQLITE_OK else {
            db = nil
            return
        }
        exec("PRAGMA journal_mode=WAL")
        exec("PRAGMA synchronous=NORMAL")
        exec("PRAGMA foreign_keys=ON")
        migrate()
        isOpen = true
    }

    private func migrate() {
        exec("""
            CREATE TABLE IF NOT EXISTS kv (
                key        TEXT PRIMARY KEY,
                value      BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            )
            """)
        exec("""
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )
            """)
        exec("""
            CREATE TABLE IF NOT EXISTS ops (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                kind       TEXT NOT NULL,
                path       TEXT NOT NULL,
                method     TEXT NOT NULL,
                body       BLOB,
                created_at INTEGER NOT NULL,
                attempts   INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )
            """)
        exec("""
            CREATE TABLE IF NOT EXISTS downloads (
                book_uuid      TEXT NOT NULL,
                kind           TEXT NOT NULL,
                format         TEXT NOT NULL,
                state          TEXT NOT NULL,
                local_path     TEXT,
                total_bytes    INTEGER NOT NULL DEFAULT 0,
                received_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at     INTEGER NOT NULL,
                error          TEXT,
                PRIMARY KEY (book_uuid, kind)
            )
            """)
        if !hasColumn("downloads", "kind") { rebuildDownloadsWithKind() }
        // Additive, so a record written before content validators existed
        // simply carries a null and reads as "can't tell".
        if !hasColumn("downloads", "source_etag") {
            exec("ALTER TABLE downloads ADD COLUMN source_etag TEXT")
        }
        // Additive too: a row written before downloads could hold more than
        // one file carries a null here and is read back through
        // `local_path` as the single file it is.
        if !hasColumn("downloads", "files") {
            exec("ALTER TABLE downloads ADD COLUMN files TEXT")
        }
        // Coalescing key for the outbox: a second position write for the same
        // book should replace the first, not queue behind it.
        exec("CREATE INDEX IF NOT EXISTS ops_kind_idx ON ops(kind)")

        // The whole library, not just whatever page happened to be on screen.
        //
        // `payload` is the encoded `Book` and is what callers actually get
        // back; every column beside it exists so a sort, a format filter, or a
        // search can run without decoding thousands of rows. They are lowercased
        // at write time so the query side needs no collation of its own.
        //
        // This is a best-effort mirror, not a byte-identical replica of the
        // server's ordering — it backs offline paging and local search, and the
        // server stays authoritative whenever it can be reached.
        exec("""
            CREATE TABLE IF NOT EXISTS books (
                uuid         TEXT PRIMARY KEY,
                title        TEXT NOT NULL DEFAULT '',
                author       TEXT NOT NULL DEFAULT '',
                series       TEXT NOT NULL DEFAULT '',
                series_index REAL NOT NULL DEFAULT 0,
                added_at     TEXT NOT NULL DEFAULT '',
                modified     TEXT NOT NULL DEFAULT '',
                formats      TEXT NOT NULL DEFAULT '',
                search_text  TEXT NOT NULL DEFAULT '',
                payload      BLOB NOT NULL
            )
            """)
        exec("CREATE INDEX IF NOT EXISTS books_title_idx ON books(title)")
        exec("CREATE INDEX IF NOT EXISTS books_author_idx ON books(author)")
        exec("CREATE INDEX IF NOT EXISTS books_added_idx ON books(added_at)")

        // Where a sync pass accumulates before it is allowed to become the
        // mirror. Pages land here one committed transaction at a time, so no
        // write lock is ever held across a network call, and an interrupted
        // pass keeps what it fetched instead of throwing it away. `books` only
        // changes at the swap, so a reader never sees a half-applied library.
        exec("""
            CREATE TABLE IF NOT EXISTS books_staging (
                uuid         TEXT PRIMARY KEY,
                title        TEXT NOT NULL DEFAULT '',
                author       TEXT NOT NULL DEFAULT '',
                series       TEXT NOT NULL DEFAULT '',
                series_index REAL NOT NULL DEFAULT 0,
                added_at     TEXT NOT NULL DEFAULT '',
                modified     TEXT NOT NULL DEFAULT '',
                formats      TEXT NOT NULL DEFAULT '',
                search_text  TEXT NOT NULL DEFAULT '',
                payload      BLOB NOT NULL
            )
            """)
    }

    /// Rebuild a pre-`kind` download registry in place.
    ///
    /// The old table was keyed on `book_uuid` alone, so it can hold at most one
    /// row per book and every one of them transfers cleanly. The stored
    /// container extension is what says which file it was.
    private func rebuildDownloadsWithKind() {
        exec("BEGIN IMMEDIATE")
        exec("""
            CREATE TABLE downloads_v2 (
                book_uuid      TEXT NOT NULL,
                kind           TEXT NOT NULL,
                format         TEXT NOT NULL,
                state          TEXT NOT NULL,
                local_path     TEXT,
                total_bytes    INTEGER NOT NULL DEFAULT 0,
                received_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at     INTEGER NOT NULL,
                error          TEXT,
                PRIMARY KEY (book_uuid, kind)
            )
            """)
        let audio = Book.audioFormats
            .map { "'\($0)'" }
            .sorted()
            .joined(separator: ",")
        exec("""
            INSERT OR REPLACE INTO downloads_v2
              (book_uuid, kind, format, state, local_path, total_bytes,
               received_bytes, updated_at, error)
            SELECT book_uuid,
                   CASE WHEN lower(format) IN (\(audio)) THEN 'audio' ELSE 'ebook' END,
                   format, state, local_path, total_bytes, received_bytes,
                   updated_at, error
            FROM downloads
            """)
        exec("DROP TABLE downloads")
        exec("ALTER TABLE downloads_v2 RENAME TO downloads")
        exec("COMMIT")
    }

    private func hasColumn(_ table: String, _ column: String) -> Bool {
        guard db != nil else { return false }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "PRAGMA table_info(\(table))", -1, &stmt, nil) == SQLITE_OK
        else { return false }
        while sqlite3_step(stmt) == SQLITE_ROW {
            if text(stmt, 1) == column { return true }
        }
        return false
    }

    // MARK: - Key/value replica cache

    func cacheGet(_ key: String) -> Data? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT value FROM kv WHERE key = ?", -1, &stmt, nil) == SQLITE_OK
        else { return nil }
        bind(stmt, 1, key)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        guard let bytes = sqlite3_column_blob(stmt, 0) else { return nil }
        let count = Int(sqlite3_column_bytes(stmt, 0))
        return Data(bytes: bytes, count: count)
    }

    func cachePut(_ key: String, _ value: Data) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            INSERT INTO kv (key, value, updated_at) VALUES (?, ?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, key)
        bind(stmt, 2, value)
        sqlite3_bind_int64(stmt, 3, Int64(Date().timeIntervalSince1970))
        sqlite3_step(stmt)
    }

    func cacheDelete(_ key: String) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "DELETE FROM kv WHERE key = ?", -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, key)
        sqlite3_step(stmt)
    }

    func cacheDeletePrefix(_ prefix: String) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "DELETE FROM kv WHERE key LIKE ? || '%'", -1, &stmt, nil) == SQLITE_OK
        else { return }
        bind(stmt, 1, prefix)
        sqlite3_step(stmt)
    }

    // MARK: - Meta

    func metaGet(_ key: String) -> String? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT value FROM meta WHERE key = ?", -1, &stmt, nil) == SQLITE_OK
        else { return nil }
        bind(stmt, 1, key)
        guard sqlite3_step(stmt) == SQLITE_ROW, let c = sqlite3_column_text(stmt, 0) else { return nil }
        return String(cString: c)
    }

    func metaDelete(_ key: String) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "DELETE FROM meta WHERE key = ?", -1, &stmt, nil) == SQLITE_OK
        else { return }
        bind(stmt, 1, key)
        sqlite3_step(stmt)
    }

    func metaPut(_ key: String, _ value: String) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            INSERT INTO meta (key, value) VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, key)
        bind(stmt, 2, value)
        sqlite3_step(stmt)
    }

    // MARK: - Outbox

    /// Queue a mutation, answering with its row id so a caller can ask later
    /// whether this particular write ever landed. `coalesce` replaces any
    /// pending op with the same kind — the reader writes a position every few
    /// seconds, and only the newest one is worth replaying.
    @discardableResult
    func enqueue(
        kind: String, path: String, method: String, body: Data?, coalesce: Bool
    ) -> Int64 {
        guard isOpen else { return 0 }
        if coalesce {
            var del: OpaquePointer?
            if sqlite3_prepare_v2(db, "DELETE FROM ops WHERE kind = ?", -1, &del, nil) == SQLITE_OK {
                bind(del, 1, kind)
                sqlite3_step(del)
            }
            sqlite3_finalize(del)
        }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            INSERT INTO ops (kind, path, method, body, created_at, attempts)
            VALUES (?, ?, ?, ?, ?, 0)
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        bind(stmt, 1, kind)
        bind(stmt, 2, path)
        bind(stmt, 3, method)
        if let body { bind(stmt, 4, body) } else { sqlite3_bind_null(stmt, 4) }
        sqlite3_bind_int64(stmt, 5, Int64(Date().timeIntervalSince1970))
        guard sqlite3_step(stmt) == SQLITE_DONE else { return 0 }
        return sqlite3_last_insert_rowid(db)
    }

    /// Whether a queued op is still there. A write that has left the queue
    /// either landed or was superseded by a coalescing write behind it — in
    /// both cases the caller's optimistic row is no longer the pending one.
    func hasOp(_ id: Int64) -> Bool {
        guard isOpen, id != 0 else { return false }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT 1 FROM ops WHERE id = ?", -1, &stmt, nil) == SQLITE_OK
        else { return false }
        sqlite3_bind_int64(stmt, 1, id)
        return sqlite3_step(stmt) == SQLITE_ROW
    }

    /// Ops still worth replaying. A row carrying `last_error` was rejected by
    /// the server in a way retrying cannot fix, so it is held for the UI rather
    /// than tried again — and excluded here so one bad op can neither wedge the
    /// queue nor keep the replica from ever revalidating.
    func listOps(limit: Int = 200) -> [PendingOp] {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT id, kind, path, method, body, created_at, attempts, last_error
            FROM ops WHERE last_error IS NULL ORDER BY id ASC LIMIT ?
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        sqlite3_bind_int(stmt, 1, Int32(limit))
        var results: [PendingOp] = []
        while sqlite3_step(stmt) == SQLITE_ROW { results.append(readOp(stmt)) }
        return results
    }

    private func readOp(_ stmt: OpaquePointer?) -> PendingOp {
        var body: Data?
        if let bytes = sqlite3_column_blob(stmt, 4) {
            body = Data(bytes: bytes, count: Int(sqlite3_column_bytes(stmt, 4)))
        }
        return PendingOp(
            id: sqlite3_column_int64(stmt, 0),
            kind: text(stmt, 1) ?? "",
            path: text(stmt, 2) ?? "",
            method: text(stmt, 3) ?? "POST",
            body: body,
            createdAt: sqlite3_column_int64(stmt, 5),
            attempts: sqlite3_column_int64(stmt, 6),
            lastError: text(stmt, 7)
        )
    }

    /// The newest queued op of one `kind`, held-aside rows included.
    ///
    /// A restore reading the outbox wants this device's last statement of the
    /// value regardless of whether it will ever land — a position parked by a
    /// terminal 4xx is still the newest place this device knows the listener
    /// reached, and `last_error IS NULL` (as `listOps` filters on) would hide
    /// exactly the row this exists to recover.
    func latestOp(kind: String) -> PendingOp? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT id, kind, path, method, body, created_at, attempts, last_error
            FROM ops WHERE kind = ? ORDER BY id DESC LIMIT 1
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return nil }
        bind(stmt, 1, kind)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        return readOp(stmt)
    }

    func pendingCount() -> Int {
        count("SELECT COUNT(*) FROM ops WHERE last_error IS NULL")
    }

    /// The distinct kinds still awaiting replay.
    ///
    /// What a replica read needs to know is not *how many* writes are queued
    /// but *which rows* they would change — a queued session report says
    /// nothing about whether the server's shelf list can be trusted. `kind`
    /// already names the target, so this is the whole input `OutboxScope` needs
    /// to answer that per key.
    func pendingKinds() -> Set<String> {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = "SELECT DISTINCT kind FROM ops WHERE last_error IS NULL"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        var kinds: Set<String> = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            if let kind = text(stmt, 0) { kinds.insert(kind) }
        }
        return kinds
    }

    /// Count one failed-but-retryable attempt, returning the new total so the
    /// caller can decide whether the op is worth another pass.
    func noteOpAttempt(_ id: Int64) -> Int64 {
        guard isOpen else { return 0 }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = "UPDATE ops SET attempts = attempts + 1 WHERE id = ? RETURNING attempts"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        sqlite3_bind_int64(stmt, 1, id)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return 0 }
        return sqlite3_column_int64(stmt, 0)
    }

    /// Ops the server refused. Surfaced on the You tab — a refused write is one
    /// the reader made and the server never took, and once it stops blocking
    /// revalidation the server's answer overwrites it, so without this the
    /// change would simply revert with nothing said about it.
    func rejectedCount() -> Int {
        count("SELECT COUNT(*) FROM ops WHERE last_error IS NOT NULL")
    }

    func rejectedOps() -> [PendingOp] {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT id, kind, path, method, body, created_at, attempts, last_error
            FROM ops WHERE last_error IS NOT NULL ORDER BY id ASC
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        var results: [PendingOp] = []
        while sqlite3_step(stmt) == SQLITE_ROW { results.append(readOp(stmt)) }
        return results
    }

    func discardRejectedOps() {
        exec("DELETE FROM ops WHERE last_error IS NOT NULL")
    }

    private func count(_ sql: String) -> Int {
        guard isOpen else { return 0 }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        guard sqlite3_step(stmt) == SQLITE_ROW else { return 0 }
        return Int(sqlite3_column_int64(stmt, 0))
    }

    func deleteOps(_ ids: [Int64]) {
        guard isOpen, !ids.isEmpty else { return }
        let placeholders = ids.map { _ in "?" }.joined(separator: ",")
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "DELETE FROM ops WHERE id IN (\(placeholders))", -1, &stmt, nil) == SQLITE_OK
        else { return }
        for (index, id) in ids.enumerated() {
            sqlite3_bind_int64(stmt, Int32(index + 1), id)
        }
        sqlite3_step(stmt)
    }

    func noteOpFailure(_ id: Int64, message: String) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = "UPDATE ops SET attempts = attempts + 1, last_error = ? WHERE id = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, message)
        sqlite3_bind_int64(stmt, 2, id)
        sqlite3_step(stmt)
    }

    /// Fail every op queued after `id` whose path names `token`.
    ///
    /// A create the server refused leaves the ops that address what it would
    /// have made with nothing to address. They are all keyed on the same
    /// client-minted id — a note PATCH and a delete both spell it into their
    /// path — so replaying them earns a 404 apiece and reports one refused
    /// change as three. Marking them here retires them against the cause
    /// instead. Bounded to ops *behind* the rejected one so a later, unrelated
    /// write that happens to reuse the id can't be caught by it.
    func failOpsReferencing(_ token: String, after id: Int64, message: String) {
        guard isOpen, !token.isEmpty else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            UPDATE ops SET last_error = ?
            WHERE id > ? AND last_error IS NULL AND instr(path, ?) > 0
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, message)
        sqlite3_bind_int64(stmt, 2, id)
        bind(stmt, 3, token)
        sqlite3_step(stmt)
    }

    func clearOps() {
        exec("DELETE FROM ops")
    }

    // MARK: - Library index

    /// One row of the local library mirror, pre-flattened for sorting and
    /// matching. `payload` is the encoded `Book`.
    struct BookRow: Sendable {
        var uuid: String
        var title: String
        var author: String
        var series: String
        var seriesIndex: Double
        var addedAt: String
        var modified: String
        var formats: String
        var searchText: String
        var payload: Data
    }

    /// Discard any half-finished sync pass.
    func clearStaging() {
        exec("DELETE FROM books_staging")
    }

    func stagedCount() -> Int {
        guard isOpen else { return 0 }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM books_staging", -1, &stmt, nil)
            == SQLITE_OK
        else { return 0 }
        guard sqlite3_step(stmt) == SQLITE_ROW else { return 0 }
        return Int(sqlite3_column_int64(stmt, 0))
    }

    /// Add one page to the pass, in its own short transaction.
    ///
    /// Deliberately not part of a transaction spanning the whole sync: the
    /// store shares a single connection, so a long-running write would enclose
    /// every other writer's work — a reading position saved mid-sync would be
    /// rolled back along with the sync if the server quit. One page, one
    /// commit, lock held for microseconds.
    func appendStagedBooks(_ rows: [BookRow]) {
        guard isOpen, !rows.isEmpty else { return }
        exec("BEGIN IMMEDIATE")
        var stmt: OpaquePointer?
        let sql = """
            INSERT INTO books_staging
              (uuid, title, author, series, series_index, added_at, modified,
               formats, search_text, payload)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(uuid) DO UPDATE SET
              title = excluded.title, author = excluded.author,
              series = excluded.series, series_index = excluded.series_index,
              added_at = excluded.added_at, modified = excluded.modified,
              formats = excluded.formats, search_text = excluded.search_text,
              payload = excluded.payload
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else {
            exec("ROLLBACK")
            return
        }
        for row in rows {
            bind(stmt, 1, row.uuid)
            bind(stmt, 2, row.title)
            bind(stmt, 3, row.author)
            bind(stmt, 4, row.series)
            sqlite3_bind_double(stmt, 5, row.seriesIndex)
            bind(stmt, 6, row.addedAt)
            bind(stmt, 7, row.modified)
            bind(stmt, 8, row.formats)
            bind(stmt, 9, row.searchText)
            bind(stmt, 10, row.payload)
            sqlite3_step(stmt)
            sqlite3_reset(stmt)
        }
        sqlite3_finalize(stmt)
        exec("COMMIT")
    }

    /// Make a completed pass the mirror.
    ///
    /// The swap is what keeps deletion honest: a book absent from a *complete*
    /// pass is genuinely gone, so `books` is replaced wholesale rather than
    /// upserted into. Doing this incrementally would mean treating "not seen
    /// yet" as "deleted" — and since the pass is keyset-ordered, a book
    /// retitled mid-sync moves behind the cursor and is never seen, so
    /// incremental deletion would drop a book that still exists.
    func promoteCompletedStaging() {
        guard isOpen else { return }
        exec("BEGIN IMMEDIATE")
        exec("DELETE FROM books")
        exec("INSERT INTO books SELECT * FROM books_staging")
        exec("DELETE FROM books_staging")
        exec("COMMIT")
    }

    /// Publish an unfinished pass without ending it.
    ///
    /// Only for the case where there is no mirror yet: a first sync that gets
    /// halfway is far more useful published than withheld, and with `books`
    /// empty there is no deletion question to get wrong. Staging is left intact
    /// so the pass can resume and later swap properly.
    func publishPartialStaging() {
        guard isOpen else { return }
        exec("BEGIN IMMEDIATE")
        exec("INSERT OR REPLACE INTO books SELECT * FROM books_staging")
        exec("COMMIT")
    }

    func bookCount() -> Int {
        guard isOpen else { return 0 }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM books", -1, &stmt, nil) == SQLITE_OK
        else { return 0 }
        guard sqlite3_step(stmt) == SQLITE_ROW else { return 0 }
        return Int(sqlite3_column_int64(stmt, 0))
    }

    /// One page of the index. `orderClause` and `whereClause` are built by
    /// `LibraryIndex` from a closed set of sorts and filters — never from user
    /// input — so they can be interpolated safely.
    func bookPayloads(
        whereClause: String,
        orderClause: String,
        bindings: [String],
        limit: Int,
        offset: Int
    ) -> [Data] {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT payload FROM books
            \(whereClause.isEmpty ? "" : "WHERE \(whereClause)")
            ORDER BY \(orderClause)
            LIMIT ? OFFSET ?
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        var index: Int32 = 1
        for value in bindings {
            bind(stmt, index, value)
            index += 1
        }
        sqlite3_bind_int(stmt, index, Int32(limit))
        sqlite3_bind_int(stmt, index + 1, Int32(offset))

        var results: [Data] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let bytes = sqlite3_column_blob(stmt, 0) else { continue }
            results.append(Data(bytes: bytes, count: Int(sqlite3_column_bytes(stmt, 0))))
        }
        return results
    }

    /// One book's encoded `Book` out of the mirror.
    ///
    /// The replica cache only holds what a screen has already asked for, so a
    /// book whose detail page has never been opened has no entry there — but
    /// the mirror holds every book in the library, this one included.
    func bookPayload(uuid: String) -> Data? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT payload FROM books WHERE uuid = ?", -1, &stmt, nil)
            == SQLITE_OK
        else { return nil }
        bind(stmt, 1, uuid)
        guard sqlite3_step(stmt) == SQLITE_ROW,
              let bytes = sqlite3_column_blob(stmt, 0)
        else { return nil }
        return Data(bytes: bytes, count: Int(sqlite3_column_bytes(stmt, 0)))
    }

    /// Total matching `whereClause`, for the "N books" readout.
    func bookMatchCount(whereClause: String, bindings: [String]) -> Int64 {
        guard isOpen else { return 0 }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT COUNT(*) FROM books
            \(whereClause.isEmpty ? "" : "WHERE \(whereClause)")
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        for (offset, value) in bindings.enumerated() {
            bind(stmt, Int32(offset + 1), value)
        }
        guard sqlite3_step(stmt) == SQLITE_ROW else { return 0 }
        return sqlite3_column_int64(stmt, 0)
    }

    // MARK: - Download registry

    func upsertDownload(_ record: DownloadRecord) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            INSERT INTO downloads
              (book_uuid, kind, format, state, local_path, total_bytes, received_bytes,
               updated_at, error, source_etag, files)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(book_uuid, kind) DO UPDATE SET
              format = excluded.format, state = excluded.state, local_path = excluded.local_path,
              total_bytes = excluded.total_bytes, received_bytes = excluded.received_bytes,
              updated_at = excluded.updated_at, error = excluded.error,
              source_etag = excluded.source_etag, files = excluded.files
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, record.bookUUID)
        bind(stmt, 2, record.kind.rawValue)
        bind(stmt, 3, record.format)
        bind(stmt, 4, record.state.rawValue)
        // Null for a multi-part record — see `DownloadRecord.localPath`.
        if let path = record.localPath { bind(stmt, 5, path) } else { sqlite3_bind_null(stmt, 5) }
        sqlite3_bind_int64(stmt, 6, record.totalBytes)
        sqlite3_bind_int64(stmt, 7, record.receivedBytes)
        sqlite3_bind_int64(stmt, 8, Int64(Date().timeIntervalSince1970))
        if let error = record.error { bind(stmt, 9, error) } else { sqlite3_bind_null(stmt, 9) }
        if let etag = record.sourceEtag { bind(stmt, 10, etag) } else { sqlite3_bind_null(stmt, 10) }
        if let files = try? JSONEncoder().encode(record.files),
           let json = String(data: files, encoding: .utf8) {
            bind(stmt, 11, json)
        } else {
            sqlite3_bind_null(stmt, 11)
        }
        sqlite3_step(stmt)
    }

    private static let downloadColumns = """
        book_uuid, kind, format, state, local_path, total_bytes, received_bytes, updated_at,
        error, source_etag, files
        """

    func download(for uuid: String, kind: DownloadKind) -> DownloadRecord? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT \(Self.downloadColumns) FROM downloads
            WHERE book_uuid = ? AND kind = ?
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return nil }
        bind(stmt, 1, uuid)
        bind(stmt, 2, kind.rawValue)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        return readDownload(stmt)
    }

    func allDownloads() -> [DownloadRecord] {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT \(Self.downloadColumns) FROM downloads ORDER BY updated_at DESC
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        var results: [DownloadRecord] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            if let record = readDownload(stmt) { results.append(record) }
        }
        return results
    }

    func deleteDownload(_ uuid: String, kind: DownloadKind) {
        guard isOpen else { return }
        if let record = download(for: uuid, kind: kind) {
            Self.removeFiles(of: record)
        }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = "DELETE FROM downloads WHERE book_uuid = ? AND kind = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, uuid)
        bind(stmt, 2, kind.rawValue)
        sqlite3_step(stmt)
    }

    /// Whether the mirror holds any book carrying one of `kind`'s formats.
    ///
    /// The question behind [`pruneDownloadsMissingFromLibrary`]: a library the
    /// server stopped reporting on is indistinguishable, row by row, from one
    /// whose books were all deleted. At the level of a whole format it is not
    /// — an ebook library that still has books while every audiobook vanished
    /// at once is a server that lost its audiobook path, not a reader who
    /// deleted their entire audio collection between two syncs.
    func libraryHasKind(_ kind: DownloadKind) -> Bool {
        guard isOpen else { return false }
        let formats = kind == .audio ? Book.audioFormats : Book.ebookFormats
        let matches = formats.map { _ in "(',' || formats || ',') LIKE ?" }
            .joined(separator: " OR ")
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = "SELECT 1 FROM books WHERE \(matches) LIMIT 1"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return false }
        for (offset, format) in formats.sorted().enumerated() {
            bind(stmt, Int32(offset + 1), "%,\(format.lowercased()),%")
        }
        return sqlite3_step(stmt) == SQLITE_ROW
    }

    /// Drop registry rows — and their files — for books the library no longer
    /// has, answering with the keys removed. Restricted to `kinds`, the
    /// formats the caller has established the mirror actually describes.
    ///
    /// A book deleted on the server leaves the mirror at the next completed
    /// sync, but its download stayed forever: invisible to the "Downloaded"
    /// filter, which joins against `books`, while still occupying the disk it
    /// reported under "Storage used".
    ///
    /// Only safe against a *complete* mirror — the caller checks. Against an
    /// empty or half-built one, every book looks deleted.
    func pruneDownloadsMissingFromLibrary(kinds: Set<DownloadKind>) -> [String] {
        guard isOpen, !kinds.isEmpty else { return [] }
        let placeholders = kinds.map { _ in "?" }.joined(separator: ",")
        let predicate = """
            book_uuid NOT IN (SELECT uuid FROM books) AND kind IN (\(placeholders))
            """
        let ordered = kinds.map(\.rawValue).sorted()

        var stmt: OpaquePointer?
        let sql = "SELECT \(Self.downloadColumns) FROM downloads WHERE \(predicate)"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        for (offset, kind) in ordered.enumerated() {
            bind(stmt, Int32(offset + 1), kind)
        }
        var orphans: [DownloadRecord] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            if let record = readDownload(stmt) { orphans.append(record) }
        }
        sqlite3_finalize(stmt)
        guard !orphans.isEmpty else { return [] }

        for record in orphans {
            Self.removeFiles(of: record)
        }
        var delete: OpaquePointer?
        defer { sqlite3_finalize(delete) }
        guard sqlite3_prepare_v2(db, "DELETE FROM downloads WHERE \(predicate)", -1, &delete, nil)
            == SQLITE_OK
        else { return [] }
        for (offset, kind) in ordered.enumerated() {
            bind(delete, Int32(offset + 1), kind)
        }
        sqlite3_step(delete)
        return orphans.map(\.id)
    }

    /// Delete every file a record points at, staged and installed alike.
    ///
    /// A multi-part audiobook holds one file per part, so deleting `localPath`
    /// alone would leave the rest on disk counting against nothing, with no
    /// row left to remove them from. The staged `incoming.` copies go the same
    /// way: these sweeps drop the row that names them, and no other path could
    /// ever find them again.
    static func removeFiles(of record: DownloadRecord) {
        for file in record.files {
            for name in file.onDiskNames {
                try? FileManager.default.removeItem(
                    at: downloadsDirectory.appendingPathComponent(name)
                )
            }
        }
    }

    private func readDownload(_ stmt: OpaquePointer?) -> DownloadRecord? {
        guard let uuid = text(stmt, 0) else { return nil }
        let format = text(stmt, 2) ?? ""
        let state = DownloadRecord.State(rawValue: text(stmt, 3) ?? "") ?? .failed
        let kind = DownloadKind(rawValue: text(stmt, 1) ?? "")
            ?? DownloadKind.inferred(fromFormat: format)
        return DownloadRecord(
            bookUUID: uuid,
            kind: kind,
            format: format,
            state: state,
            files: Self.readFiles(
                json: text(stmt, 10),
                legacyPath: text(stmt, 4),
                uuid: uuid,
                kind: kind,
                format: format,
                totalBytes: sqlite3_column_int64(stmt, 5),
                receivedBytes: sqlite3_column_int64(stmt, 6),
                state: state
            ),
            updatedAt: sqlite3_column_int64(stmt, 7),
            error: text(stmt, 8),
            sourceEtag: text(stmt, 9)
        )
    }

    /// The file list of a stored row, reconstructing it from the pre-list
    /// columns for a record written before downloads could hold more than one
    /// file. Those are all single-file by construction, so the legacy path and
    /// the row's byte counters describe it completely.
    ///
    /// `local_path` may be null even so: the build that wrote these rows only
    /// filled it *on completion*, so a download still in flight when the app
    /// updated has neither column. Its name is still derivable — it is the one
    /// that build would have installed it under — and deriving it is what lets
    /// the transfer the background session is still carrying land instead of
    /// being discarded as a failure on a perfectly good 200.
    nonisolated static func readFiles(
        json: String?,
        legacyPath: String?,
        uuid: String,
        kind: DownloadKind,
        format: String,
        totalBytes: Int64,
        receivedBytes: Int64,
        state: DownloadRecord.State
    ) -> [DownloadFile] {
        if let json, let data = json.data(using: .utf8),
           let decoded = try? JSONDecoder().decode([DownloadFile].self, from: data),
           !decoded.isEmpty {
            return decoded
        }
        // Rows written before `name` became container-relative hold a path
        // under a container UUID iOS has since reassigned. The file is still
        // there under its own name.
        let name = legacyPath.map { ($0 as NSString).lastPathComponent }
            ?? (format.isEmpty ? nil : "\(uuid).\(kind.rawValue).\(format)")
        guard let name else { return [] }
        return [
            DownloadFile(
                ordinal: 0,
                // Empty on purpose — this row never recorded a URL, and
                // `isLegacyShape` reads that back as "reconstructed".
                urlPath: "",
                name: name,
                receivedBytes: receivedBytes,
                totalBytes: totalBytes,
                done: state == .complete
            )
        ]
    }

    // MARK: - Account scoping

    /// Cache-key prefixes holding user-scoped data. Library-wide rows (books,
    /// authors, series) survive an account switch; these do not.
    ///
    /// Every entry is matched as a **prefix**, which is the trap this list
    /// keeps falling into: `"shelves"` does not cover `shelf_previews`, and
    /// neither do `"shelf:"` or `"shelf_page:"`, so the previous account's
    /// shelf names and cover artwork stayed on the Library landing rail after
    /// someone else signed in — indefinitely, on a device with no route to the
    /// server. `userScopedKeysAreCovered` in the test suite pins every key
    /// `CacheKey` can mint against this list so the next one can't slip
    /// through the same gap.
    ///
    /// A near-copy of `USER_SCOPED_PREFIXES` in `frontend/src/offline.rs` but
    /// deliberately not identical — the two clients mint different keys.
    /// `read_status:`, `shelves_with:`, and `shelf_previews` exist only here;
    /// `reader_cfi:` and `audio_pos:` only there. The overlap is kept in the
    /// same order so the two can still be read side by side.
    static let userScopedPrefixes = [
        "me", "progress:", "recent_progress", "rate:", "highlights:", "bookmarks:",
        "journals:", "rating:", "ratings_others:", "shelves", "shelf:", "shelf_page:",
        "shelf_previews", "stats:", "reader_cfi:", "audio_pos:", "audio_rate:",
        "read_status:", "shelves_with:", "syncprompt:",
    ]

    /// Record which account this device's replica belongs to; when a different
    /// user signs in, wipe the previous user's data so nothing leaks across
    /// accounts.
    func noteUser(_ username: String) {
        guard isOpen else { return }
        let previous = metaGet("last_user")
        if previous == username { return }
        if previous != nil {
            for prefix in Self.userScopedPrefixes {
                cacheDeletePrefix(prefix)
            }
            clearOps()
        }
        metaPut("last_user", username)
    }

    /// Wipe everything this device holds, for a repoint at a different server.
    ///
    /// `noteUser` keys on the username alone, which says nothing about *which*
    /// server issued it — so nothing else here would fire on a repoint, and the
    /// app would go on serving the previous instance's library, its cached
    /// pages, and download badges for files that came from somewhere else. A
    /// different server is a different library; none of it transfers.
    func resetForServerChange() {
        guard isOpen else { return }
        for record in allDownloads() {
            Self.removeFiles(of: record)
        }
        exec("BEGIN IMMEDIATE")
        exec("DELETE FROM kv")
        exec("DELETE FROM ops")
        exec("DELETE FROM downloads")
        exec("DELETE FROM books")
        exec("DELETE FROM books_staging")
        exec("DELETE FROM meta")
        exec("COMMIT")
    }

    // MARK: - sqlite plumbing

    private func exec(_ sql: String) {
        guard let db else { return }
        sqlite3_exec(db, sql, nil, nil, nil)
    }

    private func bind(_ stmt: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(stmt, index, value, -1, SQLITE_TRANSIENT)
    }

    private func bind(_ stmt: OpaquePointer?, _ index: Int32, _ value: Data) {
        // The bind result code is discarded like every other bind here: a
        // failure surfaces on the following sqlite3_step, which is where the
        // callers already handle it.
        _ = value.withUnsafeBytes { buffer in
            sqlite3_bind_blob(stmt, index, buffer.baseAddress, Int32(buffer.count), SQLITE_TRANSIENT)
        }
    }

    private func text(_ stmt: OpaquePointer?, _ column: Int32) -> String? {
        guard let c = sqlite3_column_text(stmt, column) else { return nil }
        return String(cString: c)
    }
}
