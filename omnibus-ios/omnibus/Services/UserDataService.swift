//  UserDataService.swift
//  Per-user reads and writes: progress, ratings, read status, annotations,
//  journals, shelves, and stats.
//
//  Every write is offline-tolerant: it hits the server when reachable, and
//  otherwise lands in the outbox and updates the replica optimistically so the
//  UI reflects the change immediately either way.

import Foundation

enum UserDataService {

    // MARK: - Progress

    static func progress(uuid: String) async -> ProgressRecord? {
        let record: ProgressRecord? = try? await Cache.networkFirst(CacheKey.progress(uuid)) {
            try await APIClient.shared.get("/api/progress/\(uuid)")
        }
        return record
    }

    /// Network-first: this drives the landing screen's Continue rail, and a
    /// cached empty list would keep the rail hidden even once the server has
    /// resume points. Still falls back to the replica when offline.
    static func recentProgress() async throws -> [ResumePoint] {
        try await Cache.networkFirst(CacheKey.recentProgress) {
            try await APIClient.shared.get("/api/progress/recent")
        }
    }

    /// Save a reading or listening position. Coalesced — the reader emits one
    /// of these every few seconds and only the newest matters.
    static func saveProgress(_ update: ProgressUpdate) async {
        let record = ProgressRecord(
            bookUUID: update.bookUUID,
            format: update.format,
            epubCFI: update.epubCFI,
            audioPositionSeconds: update.audioPositionSeconds,
            updatedAt: Int64(Date().timeIntervalSince1970)
        )
        await Cache.write(CacheKey.progress(update.bookUUID), record)
        do {
            let _: Empty = try await APIClient.shared.post("/api/progress", body: update)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.progress(update.bookUUID),
                path: "/api/progress",
                body: update,
                coalesce: true
            )
        }
    }

    static func reportSessions(_ reports: [SessionReport]) async {
        guard !reports.isEmpty else { return }
        do {
            let _: Empty = try await APIClient.shared.post("/api/progress/sessions", body: reports)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.session, path: "/api/progress/sessions", body: reports, coalesce: false
            )
        }
    }

    // MARK: - Ratings

    static func rating(uuid: String) async -> RatingRecord? {
        let record: RatingRecord? = try? await Cache.networkFirst(CacheKey.rating(uuid)) {
            try await APIClient.shared.get("/api/ratings/\(uuid)")
        }
        return record
    }

    static func otherRatings(uuid: String) async -> [AttributedRating] {
        let ratings: [AttributedRating]? = try? await Cache.networkFirst(
            CacheKey.ratingsOthers(uuid)
        ) {
            try await APIClient.shared.get("/api/ratings/others/\(uuid)")
        }
        return ratings ?? []
    }

    static func setRating(uuid: String, stars: Double) async {
        let update = RatingUpdate(bookUUID: uuid, stars: stars)
        await Cache.write(
            CacheKey.rating(uuid),
            RatingRecord(bookUUID: uuid, stars: stars, updatedAt: Int64(Date().timeIntervalSince1970))
        )
        do {
            let _: Empty = try await APIClient.shared.post("/api/ratings", body: update)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.rating(uuid), path: "/api/ratings", body: update, coalesce: true
            )
        }
    }

    static func clearRating(uuid: String) async {
        await OfflineStore.shared.cacheDelete(CacheKey.rating(uuid))
        do {
            let _: Empty = try await APIClient.shared.delete("/api/ratings/\(uuid)")
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.rating(uuid), path: "/api/ratings/\(uuid)",
                method: "DELETE", body: nil, coalesce: true
            )
        }
    }

    // MARK: - Read status

    static func readStatus(uuid: String) async -> ReadStatusRecord? {
        let record: ReadStatusRecord? = try? await Cache.networkFirst(CacheKey.readStatus(uuid)) {
            try await APIClient.shared.get("/api/read-status/\(uuid)")
        }
        return record
    }

    static func setReadStatus(uuid: String, status: ReadStatus) async {
        let update = SetReadStatus(bookUUID: uuid, status: status)
        await Cache.write(
            CacheKey.readStatus(uuid),
            ReadStatusRecord(
                bookUUID: uuid, status: status,
                updatedAt: Int64(Date().timeIntervalSince1970),
                finishedAt: status == .finished ? Int64(Date().timeIntervalSince1970) : nil
            )
        )
        do {
            let _: Empty = try await APIClient.shared.put("/api/read-status", body: update)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.readStatus(uuid), path: "/api/read-status",
                method: "PUT", body: update, coalesce: true
            )
        }
    }

    // MARK: - Highlights

    static func highlights(uuid: String) async -> [Highlight] {
        let items: [Highlight]? = try? await Cache.networkFirst(CacheKey.highlights(uuid)) {
            try await APIClient.shared.get("/api/highlights/book/\(uuid)")
        }
        return items ?? []
    }

    @discardableResult
    static func createHighlight(_ payload: CreateHighlight) async -> Highlight? {
        do {
            let created: Highlight = try await APIClient.shared.post("/api/highlights", body: payload)
            await refreshHighlights(payload.bookUUID)
            return created
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.highlight, path: "/api/highlights", body: payload, coalesce: false
            )
            return nil
        }
    }

    static func setHighlightColor(id: Int64, color: HighlightColor, bookUUID: String) async {
        struct Body: Encodable { let color: HighlightColor }
        do {
            let _: Empty = try await APIClient.shared.patch(
                "/api/highlights/\(id)/color", body: Body(color: color)
            )
            await refreshHighlights(bookUUID)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.highlight, path: "/api/highlights/\(id)/color",
                method: "PATCH", body: Body(color: color), coalesce: false
            )
        }
    }

    static func setHighlightNote(id: Int64, note: String?, bookUUID: String) async {
        struct Body: Encodable { let note: String? }
        do {
            let _: Empty = try await APIClient.shared.patch(
                "/api/highlights/\(id)/note", body: Body(note: note)
            )
            await refreshHighlights(bookUUID)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.highlight, path: "/api/highlights/\(id)/note",
                method: "PATCH", body: Body(note: note), coalesce: false
            )
        }
    }

    static func deleteHighlight(id: Int64, bookUUID: String) async {
        do {
            let _: Empty = try await APIClient.shared.delete("/api/highlights/\(id)")
            await refreshHighlights(bookUUID)
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.highlight, path: "/api/highlights/\(id)",
                method: "DELETE", body: nil, coalesce: false
            )
        }
    }

    private static func refreshHighlights(_ uuid: String) async {
        guard let fresh: [Highlight] = try? await APIClient.shared.get("/api/highlights/book/\(uuid)")
        else { return }
        await Cache.write(CacheKey.highlights(uuid), fresh)
    }

    // MARK: - Bookmarks

    static func bookmarks(uuid: String) async -> [Bookmark] {
        let items: [Bookmark]? = try? await Cache.networkFirst(CacheKey.bookmarks(uuid)) {
            try await APIClient.shared.get("/api/bookmarks/book/\(uuid)")
        }
        return items ?? []
    }

    @discardableResult
    static func createBookmark(_ payload: CreateBookmark) async -> Bookmark? {
        do {
            let created: Bookmark = try await APIClient.shared.post("/api/bookmarks", body: payload)
            if let fresh: [Bookmark] = try? await APIClient.shared.get("/api/bookmarks/book/\(payload.bookUUID)") {
                await Cache.write(CacheKey.bookmarks(payload.bookUUID), fresh)
            }
            return created
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.bookmark, path: "/api/bookmarks", body: payload, coalesce: false
            )
            return nil
        }
    }

    static func deleteBookmark(id: Int64, bookUUID: String) async {
        do {
            let _: Empty = try await APIClient.shared.delete("/api/bookmarks/\(id)")
            if let fresh: [Bookmark] = try? await APIClient.shared.get("/api/bookmarks/book/\(bookUUID)") {
                await Cache.write(CacheKey.bookmarks(bookUUID), fresh)
            }
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.bookmark, path: "/api/bookmarks/\(id)",
                method: "DELETE", body: nil, coalesce: false
            )
        }
    }

    // MARK: - Journals

    static func journals(uuid: String) async -> [JournalEntry] {
        let items: [JournalEntry]? = try? await Cache.networkFirst(CacheKey.journals(uuid)) {
            try await APIClient.shared.get("/api/journals/book/\(uuid)")
        }
        return items ?? []
    }

    @discardableResult
    static func createJournal(_ payload: CreateJournalEntry) async -> JournalEntry? {
        do {
            let created: JournalEntry = try await APIClient.shared.post("/api/journals", body: payload)
            if let fresh: [JournalEntry] = try? await APIClient.shared.get("/api/journals/book/\(payload.bookUUID)") {
                await Cache.write(CacheKey.journals(payload.bookUUID), fresh)
            }
            return created
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.journal, path: "/api/journals", body: payload, coalesce: false
            )
            return nil
        }
    }

    @discardableResult
    static func updateJournal(
        id: Int64, bookUUID: String, payload: UpdateJournalEntry
    ) async -> JournalEntry? {
        do {
            let updated: JournalEntry = try await APIClient.shared.patch(
                "/api/journals/\(id)", body: payload
            )
            if let fresh: [JournalEntry] = try? await APIClient.shared.get("/api/journals/book/\(bookUUID)") {
                await Cache.write(CacheKey.journals(bookUUID), fresh)
            }
            return updated
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.journal, path: "/api/journals/\(id)",
                method: "PATCH", body: payload, coalesce: false
            )
            return nil
        }
    }

    static func deleteJournal(id: Int64, bookUUID: String) async {
        do {
            let _: Empty = try await APIClient.shared.delete("/api/journals/\(id)")
            if let fresh: [JournalEntry] = try? await APIClient.shared.get("/api/journals/book/\(bookUUID)") {
                await Cache.write(CacheKey.journals(bookUUID), fresh)
            }
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.journal, path: "/api/journals/\(id)",
                method: "DELETE", body: nil, coalesce: false
            )
        }
    }

    // MARK: - Shelves

    static func shelves() async throws -> [ShelfSummary] {
        try await Cache.readThrough(CacheKey.shelves) {
            try await APIClient.shared.get("/api/shelves")
        }
    }

    /// Shelves paired with the first few covers on each.
    ///
    /// `GET /api/shelves` carries only a count, so the artwork needs one page
    /// read per shelf. They run concurrently and the whole set is cached as a
    /// unit, which keeps this to a single round of requests on a cold open and
    /// none at all on a warm one.
    static func shelfPreviews() async throws -> [ShelfPreview] {
        try await Cache.readThrough(CacheKey.shelfPreviews) {
            let shelves: [ShelfSummary] = try await APIClient.shared.get("/api/shelves")

            return await withTaskGroup(of: (Int, ShelfPreview).self) { group in
                for (index, shelf) in shelves.enumerated() {
                    group.addTask {
                        guard shelf.bookCount > 0,
                              let page: ShelfPage = try? await APIClient.shared
                                  .get("/api/shelves/\(shelf.id)/page")
                        else { return (index, ShelfPreview(shelf: shelf, covers: [])) }
                        return (
                            index,
                            ShelfPreview(shelf: shelf, covers: Array(page.books.prefix(4)))
                        )
                    }
                }
                // Task groups complete out of order; restore the server's
                // ordering so the rail doesn't reshuffle between loads.
                var collected: [(Int, ShelfPreview)] = []
                for await result in group { collected.append(result) }
                return collected.sorted { $0.0 < $1.0 }.map(\.1)
            }
        }
    }

    static func shelf(id: Int64) async throws -> Shelf {
        try await Cache.networkFirst(CacheKey.shelf(id)) {
            try await APIClient.shared.get("/api/shelves/\(id)")
        }
    }

    static func shelfBooks(id: Int64) async throws -> [Book] {
        let page: ShelfPage = try await Cache.networkFirst(CacheKey.shelfPage(id)) {
            try await APIClient.shared.get("/api/shelves/\(id)/page")
        }
        return page.books
    }

    @discardableResult
    static func createShelf(_ payload: CreateShelfRequest) async throws -> Shelf {
        let shelf: Shelf = try await APIClient.shared.post("/api/shelves", body: payload)
        await invalidateShelves()
        return shelf
    }

    static func deleteShelf(id: Int64) async throws {
        let _: Empty = try await APIClient.shared.delete("/api/shelves/\(id)")
        await invalidateShelves()
    }

    static func addBooks(shelfId: Int64, uuids: [String]) async {
        struct Body: Encodable {
            let bookUUIDs: [String]
            enum CodingKeys: String, CodingKey { case bookUUIDs = "book_uuids" }
        }
        let body = Body(bookUUIDs: uuids)
        do {
            let _: Empty = try await APIClient.shared.post("/api/shelves/\(shelfId)/books", body: body)
            await invalidateShelves()
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPage(shelfId))
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.shelfMembership, path: "/api/shelves/\(shelfId)/books",
                body: body, coalesce: false
            )
        }
    }

    static func removeBook(shelfId: Int64, uuid: String) async {
        do {
            let _: Empty = try await APIClient.shared.delete("/api/shelves/\(shelfId)/books/\(uuid)")
            await invalidateShelves()
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPage(shelfId))
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.shelfMembership, path: "/api/shelves/\(shelfId)/books/\(uuid)",
                method: "DELETE", body: nil, coalesce: false
            )
        }
    }

    /// Which shelves currently hold `uuid`.
    ///
    /// There is no "shelves containing book X" endpoint, so this reads each
    /// manual shelf's page concurrently. Smart shelves are skipped — their
    /// membership is derived and can't be toggled anyway. Never cached:
    /// showing a stale checkmark would invite a toggle that silently no-ops.
    static func shelvesContaining(uuid: String) async -> Set<Int64> {
        guard let shelves = try? await shelves() else { return [] }
        let manual = shelves.filter { $0.kind == .manual }

        return await withTaskGroup(of: Int64?.self) { group in
            for shelf in manual {
                group.addTask {
                    guard let page: ShelfPage = try? await APIClient.shared
                        .get("/api/shelves/\(shelf.id)/page")
                    else { return nil }
                    return page.books.contains { $0.uuid == uuid } ? shelf.id : nil
                }
            }
            var found: Set<Int64> = []
            for await id in group { if let id { found.insert(id) } }
            return found
        }
    }

    static func previewRule(_ request: RulePreviewRequest) async throws -> RulePreview {
        try await APIClient.shared.post("/api/shelves/preview", body: request)
    }

    private static func invalidateShelves() async {
        await OfflineStore.shared.cacheDelete(CacheKey.shelves)
        await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews)
    }

    // MARK: - Stats

    static func stats(range: StatsRange) async throws -> StatsSummary {
        try await Cache.readThrough(CacheKey.stats(range)) {
            try await APIClient.shared.get("/api/stats", query: ["range": range.rawValue])
        }
    }
}
