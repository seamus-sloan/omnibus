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

/// A book whose files have been pulled down for offline use.
struct DownloadRecord: Sendable, Identifiable {
    enum State: String, Sendable {
        case queued, running, complete, failed
    }

    var id: String { bookUUID }
    var bookUUID: String
    var format: String
    var state: State
    var localPath: String?
    var totalBytes: Int64
    var receivedBytes: Int64
    var updatedAt: Int64
    var error: String?

    var fraction: Double {
        guard totalBytes > 0 else { return state == .complete ? 1 : 0 }
        return min(1, Double(receivedBytes) / Double(totalBytes))
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
                book_uuid      TEXT PRIMARY KEY,
                format         TEXT NOT NULL,
                state          TEXT NOT NULL,
                local_path     TEXT,
                total_bytes    INTEGER NOT NULL DEFAULT 0,
                received_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at     INTEGER NOT NULL,
                error          TEXT
            )
            """)
        // Coalescing key for the outbox: a second position write for the same
        // book should replace the first, not queue behind it.
        exec("CREATE INDEX IF NOT EXISTS ops_kind_idx ON ops(kind)")
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

    func cacheAge(_ key: String) -> TimeInterval? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT updated_at FROM kv WHERE key = ?", -1, &stmt, nil) == SQLITE_OK
        else { return nil }
        bind(stmt, 1, key)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        return Date().timeIntervalSince1970 - Double(sqlite3_column_int64(stmt, 0))
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

    /// Queue a mutation. `coalesceKey` replaces any pending op with the same
    /// kind — the reader writes a position every few seconds, and only the
    /// newest one is worth replaying.
    func enqueue(kind: String, path: String, method: String, body: Data?, coalesce: Bool) {
        guard isOpen else { return }
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
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, kind)
        bind(stmt, 2, path)
        bind(stmt, 3, method)
        if let body { bind(stmt, 4, body) } else { sqlite3_bind_null(stmt, 4) }
        sqlite3_bind_int64(stmt, 5, Int64(Date().timeIntervalSince1970))
        sqlite3_step(stmt)
    }

    func listOps(limit: Int = 200) -> [PendingOp] {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT id, kind, path, method, body, created_at, attempts, last_error
            FROM ops ORDER BY id ASC LIMIT ?
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        sqlite3_bind_int(stmt, 1, Int32(limit))
        var results: [PendingOp] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            var body: Data?
            if let bytes = sqlite3_column_blob(stmt, 4) {
                body = Data(bytes: bytes, count: Int(sqlite3_column_bytes(stmt, 4)))
            }
            results.append(
                PendingOp(
                    id: sqlite3_column_int64(stmt, 0),
                    kind: text(stmt, 1) ?? "",
                    path: text(stmt, 2) ?? "",
                    method: text(stmt, 3) ?? "POST",
                    body: body,
                    createdAt: sqlite3_column_int64(stmt, 5),
                    attempts: sqlite3_column_int64(stmt, 6),
                    lastError: text(stmt, 7)
                )
            )
        }
        return results
    }

    func pendingCount() -> Int {
        guard isOpen else { return 0 }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM ops", -1, &stmt, nil) == SQLITE_OK else { return 0 }
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

    func clearOps() {
        exec("DELETE FROM ops")
    }

    // MARK: - Download registry

    func upsertDownload(_ record: DownloadRecord) {
        guard isOpen else { return }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            INSERT INTO downloads
              (book_uuid, format, state, local_path, total_bytes, received_bytes, updated_at, error)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(book_uuid) DO UPDATE SET
              format = excluded.format, state = excluded.state, local_path = excluded.local_path,
              total_bytes = excluded.total_bytes, received_bytes = excluded.received_bytes,
              updated_at = excluded.updated_at, error = excluded.error
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        bind(stmt, 1, record.bookUUID)
        bind(stmt, 2, record.format)
        bind(stmt, 3, record.state.rawValue)
        if let path = record.localPath { bind(stmt, 4, path) } else { sqlite3_bind_null(stmt, 4) }
        sqlite3_bind_int64(stmt, 5, record.totalBytes)
        sqlite3_bind_int64(stmt, 6, record.receivedBytes)
        sqlite3_bind_int64(stmt, 7, Int64(Date().timeIntervalSince1970))
        if let error = record.error { bind(stmt, 8, error) } else { sqlite3_bind_null(stmt, 8) }
        sqlite3_step(stmt)
    }

    func download(for uuid: String) -> DownloadRecord? {
        guard isOpen else { return nil }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT book_uuid, format, state, local_path, total_bytes, received_bytes, updated_at, error
            FROM downloads WHERE book_uuid = ?
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return nil }
        bind(stmt, 1, uuid)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        return readDownload(stmt)
    }

    func allDownloads() -> [DownloadRecord] {
        guard isOpen else { return [] }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        let sql = """
            SELECT book_uuid, format, state, local_path, total_bytes, received_bytes, updated_at, error
            FROM downloads ORDER BY updated_at DESC
            """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        var results: [DownloadRecord] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            if let record = readDownload(stmt) { results.append(record) }
        }
        return results
    }

    func deleteDownload(_ uuid: String) {
        guard isOpen else { return }
        if let record = download(for: uuid), let path = record.localPath {
            try? FileManager.default.removeItem(atPath: path)
        }
        var stmt: OpaquePointer?
        defer { sqlite3_finalize(stmt) }
        guard sqlite3_prepare_v2(db, "DELETE FROM downloads WHERE book_uuid = ?", -1, &stmt, nil) == SQLITE_OK
        else { return }
        bind(stmt, 1, uuid)
        sqlite3_step(stmt)
    }

    private func readDownload(_ stmt: OpaquePointer?) -> DownloadRecord? {
        guard let uuid = text(stmt, 0) else { return nil }
        return DownloadRecord(
            bookUUID: uuid,
            format: text(stmt, 1) ?? "",
            state: DownloadRecord.State(rawValue: text(stmt, 2) ?? "") ?? .failed,
            localPath: text(stmt, 3),
            totalBytes: sqlite3_column_int64(stmt, 4),
            receivedBytes: sqlite3_column_int64(stmt, 5),
            updatedAt: sqlite3_column_int64(stmt, 6),
            error: text(stmt, 7)
        )
    }

    // MARK: - Account scoping

    /// Cache-key prefixes holding user-scoped data. Library-wide rows (books,
    /// authors, series) survive an account switch; these do not. Mirrors
    /// `USER_SCOPED_PREFIXES` in `frontend/src/offline.rs`.
    private static let userScopedPrefixes = [
        "me", "progress:", "recent_progress", "rate:", "highlights:", "bookmarks:",
        "journals:", "rating:", "ratings_others:", "shelves", "shelf:", "shelf_page:",
        "stats:", "reader_cfi:", "audio_pos:", "audio_rate:", "read_status:",
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

    // MARK: - sqlite plumbing

    private func exec(_ sql: String) {
        guard let db else { return }
        sqlite3_exec(db, sql, nil, nil, nil)
    }

    private func bind(_ stmt: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(stmt, index, value, -1, SQLITE_TRANSIENT)
    }

    private func bind(_ stmt: OpaquePointer?, _ index: Int32, _ value: Data) {
        value.withUnsafeBytes { buffer in
            sqlite3_bind_blob(stmt, index, buffer.baseAddress, Int32(buffer.count), SQLITE_TRANSIENT)
        }
    }

    private func text(_ stmt: OpaquePointer?, _ column: Int32) -> String? {
        guard let c = sqlite3_column_text(stmt, column) else { return nil }
        return String(cString: c)
    }
}
