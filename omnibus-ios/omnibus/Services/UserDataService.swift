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

    /// How many resume points the Continue carousel pages through.
    ///
    /// The server defaults `?limit=` to **1**, so omitting it — which this used
    /// to do — left the carousel with a single card and its page dots dead.
    private static let resumeLimit = 5

    /// The position for one book in one format.
    ///
    /// `?format=` is not optional in practice: the server defaults it to `epub`,
    /// so an audiobook read without it silently answered with the reading
    /// position instead. `nil` is a real answer — a book nobody has opened in
    /// this format has no row, and the endpoint returns `null` for it.
    static func progress(
        uuid: String, format: ProgressFormat
    ) -> AsyncThrowingStream<CacheRead<ProgressRecord?>, Error> {
        Cache.live(CacheKey.progress(uuid, format)) {
            let record: ProgressRecord? = try await APIClient.shared.get(
                "/api/progress/\(uuid)", query: ["format": format.rawValue]
            )
            return record
        }
    }

    /// The position this device last knew about, with no network in the path.
    /// The reader opens on this; anything the server has to add arrives through
    /// `progress(uuid:format:)` afterwards.
    ///
    /// Compares the replica row against a still-queued `progress:` op and
    /// returns whichever is further along (#1860). The two can disagree: a
    /// queued op that is later lost — a terminal rejection, six retryable
    /// failures, a drained response whose body fails to decode — leaves the
    /// replica holding whatever the server answered with last, while the
    /// outbox row is still this device's own record of somewhere further on.
    /// Reading the replica alone would open there and, worse, immediately
    /// re-save it with a fresh clock — coalesce-deleting any surviving op that
    /// still remembered the real position.
    static func localProgress(uuid: String, format: ProgressFormat) async -> ProgressRecord? {
        let replica = await Cache.read(CacheKey.progress(uuid, format), as: ProgressRecord.self)
        let queued = await queuedProgress(uuid: uuid, format: format)
        return PositionSync.newest(replica, queued)
    }

    /// The still-queued `progress:` op for (uuid, format), decoded as the
    /// record it asserts. `nil` when nothing is queued, or the body no longer
    /// decodes as a `ProgressUpdate` — the shape every write to this kind
    /// encodes.
    private static func queuedProgress(
        uuid: String, format: ProgressFormat
    ) async -> ProgressRecord? {
        guard let op = await OfflineStore.shared.latestOp(kind: OpKind.progress(uuid, format)),
              let body = op.body,
              var update = try? JSONDecoder().decode(ProgressUpdate.self, from: body)
        else { return nil }
        // `clientUpdatedAt` defaults to `Date()` in `ProgressUpdate`'s own
        // decoding, so a body queued before the field existed — or any body
        // that simply omits it — would otherwise decode as "just now" and
        // make a stale op look newest, which is the exact failure this
        // restore exists to prevent. `op.createdAt`, the outbox's own enqueue
        // timestamp, is what the write actually happened on when the field
        // itself is missing.
        if !Self.bodyHasClientUpdatedAt(body) {
            update.clientUpdatedAt = op.createdAt
        }
        return update.asRecord
    }

    /// Whether a queued op's raw body carries `client_updated_at`, as opposed
    /// to `ProgressUpdate`'s decoder silently filling the gap with now.
    static func bodyHasClientUpdatedAt(_ body: Data) -> Bool {
        guard let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any]
        else { return false }
        return object["client_updated_at"] != nil
    }

    static func recentProgress() -> AsyncThrowingStream<CacheRead<[ResumePoint]>, Error> {
        Cache.live(CacheKey.recentProgress) {
            try await APIClient.shared.get(
                "/api/progress/recent", query: ["limit": String(resumeLimit)]
            )
        }
    }

    /// Save a reading or listening position. Coalesced — the reader emits one
    /// of these every few seconds and only the newest matters.
    ///
    /// The replica and the resume rail move first, then the write goes through
    /// the outbox. What comes back from the server — which may be a *different*
    /// position, if another device read further and this one was offline —
    /// lands via `SyncEngine`'s response hook.
    ///
    /// `push` spends a round trip on this particular save. The periodic ones
    /// don't: they queue, coalescing onto whatever is already there, and the
    /// moments that matter for handoff — closing the book, backgrounding,
    /// opening another — pass `true`. See `SyncEngine.record`.
    static func saveProgress(_ update: ProgressUpdate, push: Bool = true) async {
        let key = CacheKey.progress(update.bookUUID, update.format)
        let optimistic = ProgressRecord(
            bookUUID: update.bookUUID,
            format: update.format,
            epubCFI: update.epubCFI,
            audioPositionSeconds: update.audioPositionSeconds,
            progressPercent: update.progressPercent,
            // The device clock on both, because that is what this write will
            // be ordered on when it reaches the server. Leaving `clientUpdatedAt`
            // unset here would make the optimistic row the one thing in the
            // replica that couldn't be compared against a server answer.
            updatedAt: update.clientUpdatedAt,
            clientUpdatedAt: update.clientUpdatedAt,
            bookFileID: update.bookFileID
        )
        await Cache.write(key, optimistic)
        await noteResumePoint(optimistic)

        let kind = OpKind.progress(update.bookUUID, update.format)
        if push {
            await SyncEngine.shared.write(
                kind: kind, path: "/api/progress", body: update, coalesce: true
            )
        } else {
            await SyncEngine.shared.record(
                kind: kind, path: "/api/progress", body: update, coalesce: true
            )
        }
    }

    /// Move a book to the front of the cached Continue rail.
    ///
    /// The rail is a server read, and while a position write is queued the
    /// outbox holds off revalidating it — correctly, since the server's answer
    /// predates the write. But nothing wrote the local side, so reading offline
    /// left the rail showing where you were *before* the session, and a book
    /// started offline never appeared on it at all. Splicing here is what makes
    /// the rail move without the network, the same way the shelf lists already
    /// did.
    static func noteResumePoint(_ record: ProgressRecord) async {
        let points: [ResumePoint] = await Cache.cachedOnly(CacheKey.recentProgress) ?? []
        // Only a card the rail has never held needs the book resolved.
        let book = points.contains { $0.matches(record) }
            ? nil
            : await bookForResume(record.bookUUID)
        guard let spliced = splice(record, book: book, into: points) else { return }
        await Cache.write(CacheKey.recentProgress, spliced)
    }

    /// Put `record`'s card at the front of `points`, replacing the existing one
    /// for the same **(book, format)** pair.
    ///
    /// Matching on the pair rather than the book is the whole point: a
    /// dual-format book has a reading card and a listening card, and on the uuid
    /// alone a saved listening position rewrote the reading one in place — same
    /// book, wrong verb, and the reading position it had been holding was gone.
    ///
    /// `book` is consulted only when no such card exists yet. `nil` for a book
    /// that isn't already on the rail returns `nil`: better to leave the rail
    /// alone than to add a card with no metadata to draw.
    static func splice(
        _ record: ProgressRecord, book: Book?, into points: [ResumePoint]
    ) -> [ResumePoint]? {
        var points = points
        var point: ResumePoint
        if let existing = points.firstIndex(where: { $0.matches(record) }) {
            // Keep the audio duration and chapter readout the server computed —
            // this device can't derive them, and dropping them would blank the
            // card's progress bar on every save.
            point = points[existing]
            point.record = record
            points.remove(at: existing)
        } else {
            guard let book else { return nil }
            point = ResumePoint(
                record: record, book: book, totalDurationSeconds: nil,
                chapterNumber: nil, chapterCount: nil
            )
        }
        points.insert(point, at: 0)
        return Array(points.prefix(resumeLimit))
    }

    /// The book behind a resume card. The mirror holds every book in the
    /// library, so this answers for one whose detail screen has never been
    /// opened — which is exactly the book a first offline session is about.
    private static func bookForResume(_ uuid: String) async -> Book? {
        if let cached: Book = await Cache.cachedOnly(CacheKey.book(uuid)) { return cached }
        return await LibraryIndex.shared.book(uuid: uuid)
    }

    static func reportSessions(_ reports: [SessionReport]) async {
        guard !reports.isEmpty else { return }
        await SyncEngine.shared.write(
            kind: OpKind.session, path: "/api/progress/sessions", body: reports, coalesce: false
        )
    }

    // MARK: - Ratings

    static func rating(uuid: String) -> AsyncThrowingStream<CacheRead<RatingRecord>, Error> {
        Cache.live(CacheKey.rating(uuid)) {
            try await APIClient.shared.get("/api/ratings/\(uuid)")
        }
    }

    static func otherRatings(
        uuid: String
    ) -> AsyncThrowingStream<CacheRead<[AttributedRating]>, Error> {
        Cache.live(CacheKey.ratingsOthers(uuid)) {
            try await APIClient.shared.get("/api/ratings/others/\(uuid)")
        }
    }

    static func setRating(uuid: String, stars: Double) async {
        let update = RatingUpdate(bookUUID: uuid, stars: stars)
        await Cache.write(
            CacheKey.rating(uuid),
            RatingRecord(bookUUID: uuid, stars: stars, updatedAt: Int64(Date().timeIntervalSince1970))
        )
        await SyncEngine.shared.write(
            kind: OpKind.rating(uuid), path: "/api/ratings", body: update, coalesce: true
        )
    }

    static func clearRating(uuid: String) async {
        await OfflineStore.shared.cacheDelete(CacheKey.rating(uuid))
        await SyncEngine.shared.write(
            kind: OpKind.rating(uuid), path: "/api/ratings/\(uuid)",
            method: "DELETE", body: nil, coalesce: true
        )
    }

    // MARK: - Read status

    static func readStatus(
        uuid: String
    ) -> AsyncThrowingStream<CacheRead<ReadStatusRecord>, Error> {
        Cache.live(CacheKey.readStatus(uuid)) {
            try await APIClient.shared.get("/api/read-status/\(uuid)")
        }
    }

    /// The stored status the readers' auto transitions decide against.
    ///
    /// The replica answers when a queued write makes it the newest truth, and
    /// again when the server can't be asked; otherwise the server does, where
    /// `null` is a real answer — a book nobody has marked is `unread`. (The
    /// `readStatus` stream can't carry that distinction: it decodes the record
    /// non-optionally, so "no row" and "no answer" fail alike.) `nil` here
    /// means nothing is known, and the caller must stay inert — acting on a
    /// guess could downgrade an unfetched `finished`.
    static func storedReadStatus(uuid: String) async -> ReadStatus? {
        let key = CacheKey.readStatus(uuid)
        let cached: ReadStatusRecord? = await Cache.cachedOnly(key)
        // Same guard as `Cache.live`'s revalidation: with a write queued for
        // this book, the server's answer predates the replica.
        if await Cache.isBlocked(key) { return cached?.status }
        do {
            let fresh: ReadStatusRecord? = try await APIClient.shared.get(
                "/api/read-status/\(uuid)"
            )
            return fresh?.status ?? .unread
        } catch {
            return cached?.status
        }
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
        await SyncEngine.shared.write(
            kind: OpKind.readStatus(uuid), path: "/api/read-status",
            method: "PUT", body: update, coalesce: true
        )
    }

    // MARK: - Highlights

    static func highlights(uuid: String) -> AsyncThrowingStream<CacheRead<[Highlight]>, Error> {
        Cache.live(CacheKey.highlights(uuid)) {
            try await APIClient.shared.get("/api/highlights/book/\(uuid)")
        }
    }

    /// The annotations this device already holds. The reader draws these on
    /// first paint and folds in anything the server adds afterwards.
    static func localHighlights(uuid: String) async -> [Highlight] {
        await Cache.cachedOnly(CacheKey.highlights(uuid)) ?? []
    }

    /// Create a highlight, offline or not.
    ///
    /// The optimistic row goes into the replica *before* the request, so a
    /// highlight made offline is still there after a relaunch. Writing it only
    /// on the success path is what used to make an offline highlight vanish the
    /// moment the book was closed — it looked fine until then, because the
    /// reader had drawn it into the web view from its own state.
    ///
    /// The caller always gets the local row: it carries the minted `client_id`,
    /// which is what every later op addresses, so a note can hang off the
    /// passage before the create has reached the server at all.
    @discardableResult
    static func createHighlight(_ payload: CreateHighlight) async -> Highlight? {
        let optimistic = payload.optimistic
        await mutateLocalHighlights(payload.bookUUID) { $0.append(optimistic) }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.highlight, path: "/api/highlights", body: payload, coalesce: false
        )
        if landed { await refreshHighlights(payload.bookUUID) }
        return optimistic
    }

    static func setHighlightColor(_ highlight: Highlight, color: HighlightColor) async {
        struct Body: Encodable { let color: HighlightColor }
        let bookUUID = highlight.bookUUID
        await mutateLocalHighlights(bookUUID) { list in
            guard let index = list.firstIndex(where: { $0.id == highlight.id }) else { return }
            list[index].color = color
        }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.highlight, path: "/api/highlights/\(highlight.pathID)/color",
            method: "PATCH", body: Body(color: color), coalesce: false
        )
        if landed { await refreshHighlights(bookUUID) }
    }

    static func setHighlightNote(_ highlight: Highlight, note: String?) async {
        struct Body: Encodable { let note: String? }
        let bookUUID = highlight.bookUUID
        await mutateLocalHighlights(bookUUID) { list in
            guard let index = list.firstIndex(where: { $0.id == highlight.id }) else { return }
            list[index].note = note
        }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.highlight, path: "/api/highlights/\(highlight.pathID)/note",
            method: "PATCH", body: Body(note: note), coalesce: false
        )
        if landed { await refreshHighlights(bookUUID) }
    }

    static func deleteHighlight(_ highlight: Highlight) async {
        let bookUUID = highlight.bookUUID
        await mutateLocalHighlights(bookUUID) { $0.removeAll { $0.id == highlight.id } }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.highlight, path: "/api/highlights/\(highlight.pathID)",
            method: "DELETE", body: nil, coalesce: false
        )
        if landed { await refreshHighlights(bookUUID) }
    }

    /// Whether a server list may replace the cached one.
    ///
    /// Same reasoning as `Cache.live`'s guard: with writes still queued the
    /// server's list is missing them by definition, so adopting it drops rows
    /// the outbox hasn't landed. These refreshes run right after a *successful*
    /// mutation, which is exactly when an earlier op can still be waiting.
    ///
    /// Scoped per key for the same reason as that guard — a queued position
    /// write says nothing about whether a highlight list can be trusted.
    ///
    /// Asked twice per refresh, before the request and again before the write.
    /// Checking only up front proves the queue was clear when the request went
    /// out, which is not the question — the question is whether it is clear
    /// when the answer replaces the replica, and a gesture made while the
    /// request was in flight sits exactly in between.
    private static func canAdoptServerState(for key: String) async -> Bool {
        !OutboxScope.isBlocked(key, by: await OfflineStore.shared.pendingKinds())
    }

    private static func refreshHighlights(_ uuid: String) async {
        let key = CacheKey.highlights(uuid)
        guard await canAdoptServerState(for: key),
              let fresh: [Highlight] = try? await APIClient.shared.get("/api/highlights/book/\(uuid)"),
              await canAdoptServerState(for: key)
        else { return }
        await Cache.write(key, fresh)
    }

    /// Apply a change to the cached highlight list for a book.
    ///
    /// This is what keeps the replica honest between the gesture and the sync:
    /// every write goes through it, so the list on disk always reflects what
    /// the reader last saw, whether or not the server has heard about it.
    private static func mutateLocalHighlights(
        _ uuid: String, _ mutate: (inout [Highlight]) -> Void
    ) async {
        var list: [Highlight] = await Cache.cachedOnly(CacheKey.highlights(uuid)) ?? []
        mutate(&list)
        await Cache.write(CacheKey.highlights(uuid), list)
    }

    // MARK: - Bookmarks

    static func bookmarks(uuid: String) -> AsyncThrowingStream<CacheRead<[Bookmark]>, Error> {
        Cache.live(CacheKey.bookmarks(uuid)) {
            try await APIClient.shared.get("/api/bookmarks/book/\(uuid)")
        }
    }

    static func localBookmarks(uuid: String) async -> [Bookmark] {
        await Cache.cachedOnly(CacheKey.bookmarks(uuid)) ?? []
    }

    @discardableResult
    static func createBookmark(_ payload: CreateBookmark) async -> Bookmark? {
        let optimistic = payload.optimistic
        await mutateLocalBookmarks(payload.bookUUID) { $0.append(optimistic) }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.bookmark, path: "/api/bookmarks", body: payload, coalesce: false
        )
        if landed { await refreshBookmarks(payload.bookUUID) }
        return optimistic
    }

    static func deleteBookmark(_ bookmark: Bookmark) async {
        let bookUUID = bookmark.bookUUID
        await mutateLocalBookmarks(bookUUID) { $0.removeAll { $0.id == bookmark.id } }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.bookmark, path: "/api/bookmarks/\(bookmark.pathID)",
            method: "DELETE", body: nil, coalesce: false
        )
        if landed { await refreshBookmarks(bookUUID) }
    }

    private static func refreshBookmarks(_ uuid: String) async {
        let key = CacheKey.bookmarks(uuid)
        guard await canAdoptServerState(for: key),
              let fresh: [Bookmark] = try? await APIClient.shared.get("/api/bookmarks/book/\(uuid)"),
              await canAdoptServerState(for: key)
        else { return }
        await Cache.write(key, fresh)
    }

    private static func mutateLocalBookmarks(
        _ uuid: String, _ mutate: (inout [Bookmark]) -> Void
    ) async {
        var list: [Bookmark] = await Cache.cachedOnly(CacheKey.bookmarks(uuid)) ?? []
        mutate(&list)
        await Cache.write(CacheKey.bookmarks(uuid), list)
    }

    // MARK: - Journals

    static func journals(uuid: String) -> AsyncThrowingStream<CacheRead<[JournalEntry]>, Error> {
        Cache.live(CacheKey.journals(uuid)) {
            try await APIClient.shared.get("/api/journals/book/\(uuid)")
        }
    }

    @discardableResult
    static func createJournal(
        _ payload: CreateJournalEntry, author: UserSummary?
    ) async -> JournalEntry? {
        let optimistic = optimisticJournalEntry(for: payload, author: author)
        await mutateLocalJournals(payload.bookUUID) { $0.insert(optimistic, at: 0) }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.journal, path: "/api/journals", body: payload, coalesce: false
        )
        if landed { await refreshJournals(payload.bookUUID) }
        return optimistic
    }

    /// The optimistic record shown the instant a journal entry is created,
    /// before the outbox has landed the write or the server has rendered the
    /// markdown. Display name + avatar match what the server's re-read will
    /// say, so the card doesn't flicker to a different identity; a local
    /// markdown renderer here would only disagree with the real one, so the
    /// body stays as the source until the server has rendered it.
    static func optimisticJournalEntry(
        for payload: CreateJournalEntry, author: UserSummary?
    ) -> JournalEntry {
        JournalEntry(
            id: AnnotationID.pending(),
            bookUUID: payload.bookUUID,
            authorId: author?.id ?? 0,
            authorName: author?.display ?? "You",
            authorHasAvatar: author?.hasAvatar ?? false,
            bodyMd: payload.bodyMd,
            bodyHtml: "",
            progress: payload.progress,
            status: payload.status,
            clientID: payload.clientID,
            createdAt: Int64(Date().timeIntervalSince1970),
            updatedAt: Int64(Date().timeIntervalSince1970)
        )
    }

    static func updateJournal(_ entry: JournalEntry, payload: UpdateJournalEntry) async {
        let bookUUID = entry.bookUUID
        await mutateLocalJournals(bookUUID) { list in
            guard let index = list.firstIndex(where: { $0.id == entry.id }) else { return }
            list[index].bodyMd = payload.bodyMd
            list[index].progress = payload.progress
            if let status = payload.status { list[index].status = status }
            list[index].updatedAt = Int64(Date().timeIntervalSince1970)
        }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.journal, path: "/api/journals/\(entry.pathID)",
            method: "PATCH", body: payload, coalesce: false
        )
        // The refresh is what replaces the locally-patched row with the one the
        // server rendered the markdown for.
        if landed { await refreshJournals(bookUUID) }
    }

    static func deleteJournal(_ entry: JournalEntry) async {
        let bookUUID = entry.bookUUID
        await mutateLocalJournals(bookUUID) { $0.removeAll { $0.id == entry.id } }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.journal, path: "/api/journals/\(entry.pathID)",
            method: "DELETE", body: nil, coalesce: false
        )
        if landed { await refreshJournals(bookUUID) }
    }

    private static func refreshJournals(_ uuid: String) async {
        let key = CacheKey.journals(uuid)
        guard await canAdoptServerState(for: key),
              let fresh: [JournalEntry] = try? await APIClient.shared.get("/api/journals/book/\(uuid)"),
              await canAdoptServerState(for: key)
        else { return }
        await Cache.write(key, fresh)
    }

    private static func mutateLocalJournals(
        _ uuid: String, _ mutate: (inout [JournalEntry]) -> Void
    ) async {
        var list: [JournalEntry] = await Cache.cachedOnly(CacheKey.journals(uuid)) ?? []
        mutate(&list)
        await Cache.write(CacheKey.journals(uuid), list)
    }

    // MARK: - Shelves

    static func shelves() -> AsyncThrowingStream<CacheRead<[ShelfSummary]>, Error> {
        Cache.live(CacheKey.shelves) {
            try await APIClient.shared.get("/api/shelves")
        }
    }

    /// Shelves paired with the first few covers on each.
    ///
    /// `GET /api/shelves` carries only a count, so the artwork needs one page
    /// read per shelf. They run concurrently and the whole set is cached as a
    /// unit, which keeps this to a single round of requests on a cold open and
    /// none at all on a warm one.
    static func shelfPreviews() -> AsyncThrowingStream<CacheRead<[ShelfPreview]>, Error> {
        Cache.live(CacheKey.shelfPreviews) {
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

    static func shelf(id: Int64) -> AsyncThrowingStream<CacheRead<Shelf>, Error> {
        Cache.live(CacheKey.shelf(id)) {
            try await APIClient.shared.get("/api/shelves/\(id)")
        }
    }

    static func shelfBooks(id: Int64) -> AsyncThrowingStream<CacheRead<ShelfPage>, Error> {
        Cache.live(CacheKey.shelfPage(id)) {
            try await APIClient.shared.get("/api/shelves/\(id)/page")
        }
    }

    /// Create a shelf. The one write here that genuinely needs the server.
    ///
    /// A shelf has no client-minted identity the way an annotation does, and
    /// everything that follows a create — adding books, a smart shelf's rule —
    /// is keyed on the id the server assigns. Queueing the create would mean
    /// queueing every subsequent op against an id that doesn't exist yet, so
    /// this surfaces the failure instead of pretending.
    @discardableResult
    static func createShelf(_ payload: CreateShelfRequest) async throws -> Shelf {
        let shelf: Shelf = try await APIClient.shared.post("/api/shelves", body: payload)
        await invalidateShelves()
        return shelf
    }

    /// Delete a shelf. Unlike a create this has a real server id to name, so it
    /// queues like any other write and applies on reconnect.
    static func deleteShelf(id: Int64) async {
        await OfflineStore.shared.cacheDelete(CacheKey.shelf(id))
        await OfflineStore.shared.cacheDelete(CacheKey.shelfPage(id))
        // Splice the shelf out of the cached lists rather than dropping them.
        // Dropping was fine online, where a refetch follows immediately, but
        // offline it left the Shelves tab with no replica and nothing to fetch
        // — so deleting one shelf blanked all of them, and the empty state read
        // as "you have no shelves" until the device reconnected.
        await removeShelfLocally(id)
        let landed = await SyncEngine.shared.write(
            kind: OpKind.shelfMembership, path: "/api/shelves/\(id)",
            method: "DELETE", body: nil, coalesce: false
        )
        if landed { await invalidateShelves() }
    }

    private static func removeShelfLocally(_ id: Int64) async {
        await Cache.mutate(CacheKey.shelves) { (shelves: inout [ShelfSummary]) in
            shelves.removeAll { $0.id == id }
        }
        await Cache.mutate(CacheKey.shelfPreviews) { (previews: inout [ShelfPreview]) in
            previews.removeAll { $0.shelf.id == id }
        }
    }

    static func addBooks(shelfId: Int64, uuids: [String]) async {
        struct Body: Encodable {
            let bookUUIDs: [String]
            enum CodingKeys: String, CodingKey { case bookUUIDs = "book_uuids" }
        }
        let body = Body(bookUUIDs: uuids)
        // Before the request, so the checkmark is right whether or not it
        // lands. Without this an add made offline read as unchecked on the next
        // visit, inviting a second tap that queues a duplicate.
        for uuid in uuids {
            await mutateShelfMembership(uuid) { $0.insert(shelfId) }
        }
        await addBooksLocally(shelfId: shelfId, uuids: uuids)
        let landed = await SyncEngine.shared.write(
            kind: OpKind.shelfMembership, path: "/api/shelves/\(shelfId)/books",
            body: body, coalesce: false
        )
        if landed {
            await invalidateShelves()
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPage(shelfId))
        }
    }

    /// Put the books on the cached shelf page and bump the counts.
    ///
    /// The remove path has spliced the page since it was written; the add path
    /// only ever ticked the checkmark, so a book added offline was checked on
    /// its own screen and absent from the shelf it had just been added to.
    private static func addBooksLocally(shelfId: Int64, uuids: [String]) async {
        var added: [Book] = []
        for uuid in uuids {
            guard let book = await bookForResume(uuid) else { continue }
            added.append(book)
        }
        guard !added.isEmpty else { return }
        await Cache.mutate(CacheKey.shelfPage(shelfId)) { (page: inout ShelfPage) in
            let known = Set(page.books.map(\.uuid))
            page.books.append(contentsOf: added.filter { !known.contains($0.uuid) })
        }
        await adjustShelfCount(shelfId, by: added.count)
    }

    static func removeBook(shelfId: Int64, uuid: String) async {
        await mutateShelfMembership(uuid) { $0.remove(shelfId) }
        // The page can be filtered locally, so a shelf still reads correctly
        // offline instead of being dropped and coming back empty.
        var removed = false
        await Cache.mutate(CacheKey.shelfPage(shelfId)) { (page: inout ShelfPage) in
            let before = page.books.count
            page.books.removeAll { $0.uuid == uuid }
            removed = page.books.count < before
        }
        if removed { await adjustShelfCount(shelfId, by: -1) }
        let landed = await SyncEngine.shared.write(
            kind: OpKind.shelfMembership, path: "/api/shelves/\(shelfId)/books/\(uuid)",
            method: "DELETE", body: nil, coalesce: false
        )
        if landed {
            await invalidateShelves()
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPage(shelfId))
        }
    }

    /// Keep the "N books" on the shelf lists in step with a membership change,
    /// so a shelf edited offline doesn't read as the count it had beforehand.
    private static func adjustShelfCount(_ shelfId: Int64, by delta: Int) async {
        await Cache.mutate(CacheKey.shelves) { (shelves: inout [ShelfSummary]) in
            guard let index = shelves.firstIndex(where: { $0.id == shelfId }) else { return }
            shelves[index].bookCount = max(0, shelves[index].bookCount + Int64(delta))
        }
        await Cache.mutate(CacheKey.shelfPreviews) { (previews: inout [ShelfPreview]) in
            guard let index = previews.firstIndex(where: { $0.shelf.id == shelfId }) else { return }
            previews[index].shelf.bookCount = max(0, previews[index].shelf.bookCount + Int64(delta))
        }
    }

    private static func mutateShelfMembership(
        _ uuid: String, _ mutate: (inout Set<Int64>) -> Void
    ) async {
        let key = CacheKey.shelvesContaining(uuid)
        guard var ids: [Int64] = await Cache.cachedOnly(key) else { return }
        var set = Set(ids)
        mutate(&set)
        ids = set.sorted()
        await Cache.write(key, ids)
    }

    /// Which shelves currently hold `uuid`.
    ///
    /// One request. This used to fan out over every visible shelf's page — a
    /// request per shelf, on every book screen opened — because the server had
    /// no endpoint for the question. It does now; the fan-out survives only as
    /// the fallback for a server too old to answer, since the app is deployed
    /// separately from the instance it points at.
    ///
    /// This also used to refuse to cache, on the grounds that a stale checkmark
    /// invites a toggle that silently no-ops. But refusing didn't avoid the
    /// stale answer, it produced a *worse* one: offline the whole set came back
    /// empty, so every shelf the book was already on read as unchecked, and
    /// tapping one queued an add that the server would reject as a duplicate.
    /// A remembered answer is right far more often than a blank one, and the
    /// live read corrects it a moment later.
    static func shelvesContaining(uuid: String) -> AsyncThrowingStream<CacheRead<[Int64]>, Error> {
        Cache.live(CacheKey.shelvesContaining(uuid)) {
            do {
                return try await APIClient.shared.get("/api/shelves/containing/\(uuid)")
            } catch APIError.http(status: 404, _) {
                return await shelvesContainingByScan(uuid: uuid)
            }
        }
    }

    /// Pre-endpoint fallback: read every manual shelf's page and look. Smart
    /// shelves are skipped — their membership is derived and can't be toggled.
    private static func shelvesContainingByScan(uuid: String) async -> [Int64] {
        guard let all: [ShelfSummary] = try? await APIClient.shared.get("/api/shelves") else {
            return []
        }
        let manual = all.filter { $0.kind == .manual }

        return await withTaskGroup(of: Int64?.self) { group in
            for shelf in manual {
                group.addTask {
                    guard let page: ShelfPage = try? await APIClient.shared
                        .get("/api/shelves/\(shelf.id)/page")
                    else { return nil }
                    return page.books.contains { $0.uuid == uuid } ? shelf.id : nil
                }
            }
            var found: [Int64] = []
            for await id in group { if let id { found.append(id) } }
            return found.sorted()
        }
    }

    static func previewRule(_ request: RulePreviewRequest) async throws -> RulePreview {
        try await APIClient.shared.post("/api/shelves/preview", body: request)
    }

    private static func invalidateShelves() async {
        await OfflineStore.shared.cacheDelete(CacheKey.shelves)
        await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews)
    }

    // MARK: - Wishlist

    /// The caller's wishlist entry for a book, or `nil` when not tracked.
    /// Read-only here: wishlist writes fail rule 08's test 3 (the payload
    /// comes from the server's lookup), so they never queue offline.
    static func wishlistEntry(uuid: String) -> AsyncThrowingStream<CacheRead<WishlistEntry?>, Error> {
        Cache.live(CacheKey.wishlistEntry(uuid)) {
            try await APIClient.shared.get("/api/physical/\(uuid)/wishlist")
        }
    }

    // MARK: - Stats

    static func stats(range: StatsRange) -> AsyncThrowingStream<CacheRead<StatsSummary>, Error> {
        Cache.live(CacheKey.stats(range)) {
            try await APIClient.shared.get("/api/stats", query: ["range": range.rawValue])
        }
    }

    // MARK: - Offline prefetch

    /// Pull down everything a book needs to be *usable* with no network: where
    /// you are in it, what you've marked in it, and what you've written about
    /// it.
    ///
    /// Taking a book offline used to mean the file and its cover, and nothing
    /// else. The replica only holds what a screen has already asked for, so
    /// for a book downloaded from the shelf and never opened on this device
    /// that was nothing — download it, go offline, open it, and it started at
    /// the beginning with no highlights, however far along you were on another
    /// device. The file was the one part that had never been the problem.
    ///
    /// Best-effort by construction: each leg is a `live` read whose failure is
    /// dropped, so a server that goes away mid-prefetch costs only whatever it
    /// hadn't reached yet. Runs alongside the transfer rather than gating it.
    static func prefetchForOffline(uuid: String) async {
        await withTaskGroup(of: Void.self) { group in
            group.addTask { for await _ in progress(uuid: uuid, format: .epub).values() {} }
            group.addTask { for await _ in progress(uuid: uuid, format: .audio).values() {} }
            group.addTask { for await _ in highlights(uuid: uuid).values() {} }
            group.addTask { for await _ in bookmarks(uuid: uuid).values() {} }
            group.addTask { for await _ in journals(uuid: uuid).values() {} }
            group.addTask { for await _ in readStatus(uuid: uuid).values() {} }
        }
    }
}

// MARK: - Cross-format sync (rule 08: configuration-shaped — never queued)

extension UserDataService {
    /// The mapped "resume in the other format" candidate. A plain network
    /// read — no cache, no offline fallback: a prompt that can't be
    /// fetched is a prompt that doesn't appear.
    static func crossFormatResume(
        uuid: String,
        target: ProgressFormat
    ) async throws -> CrossFormatResume {
        try await APIClient.shared.get(
            "/api/books/\(uuid)/cross-format-resume",
            query: ["target": target.rawValue]
        )
    }

    /// Alignment payload for the sheet — network-only, like the resume read.
    static func alignment(uuid: String) async throws -> AlignmentView {
        try await APIClient.shared.get("/api/books/\(uuid)/alignment")
    }

    /// Confirm (or re-confirm) the cross-format link. Rule 08 test 1: this
    /// is per-user configuration — a deliberate declaration versioned by
    /// the server-side audio snapshot — so it calls the API directly and
    /// fails visibly offline; it must never queue.
    static func confirmCrossFormatLink(
        uuid: String,
        mode: CrossFormatLinkMode,
        primaryBookFileID: Int64?
    ) async throws {
        let body = ConfirmCrossFormatLink(
            bookUUID: uuid,
            mode: mode,
            primaryBookFileID: primaryBookFileID,
            audioOrder: nil
        )
        let _: Empty = try await APIClient.shared.post(
            "/api/books/\(uuid)/cross-format-link",
            body: body
        )
    }

    /// Turn sync off. Same never-queued contract as the confirm.
    static func unlinkCrossFormat(uuid: String) async throws {
        let _: Empty = try await APIClient.shared.delete("/api/books/\(uuid)/cross-format-link")
    }

    /// A "synced here" declaration: records the declaring surface's own
    /// position as a user anchor and turns follow mode on. Rule 08: a
    /// deferred declaration would calibrate positions that no longer
    /// correspond — direct call only, disabled offline at the control.
    static func declareSyncPoint(_ decl: DeclareSyncPoint) async throws {
        let _: Empty = try await APIClient.shared.post(
            "/api/books/\(decl.bookUUID)/sync-point",
            body: decl
        )
    }
}
