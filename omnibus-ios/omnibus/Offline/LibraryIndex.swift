//  LibraryIndex.swift
//  The whole library, held locally.
//
//  The replica cache holds whatever a screen last asked for; this holds the
//  library itself — every book, not the sixty that fit on the first page. It is
//  what makes paging work with no network and what search runs against, and it
//  is refreshed as a unit rather than a page at a time.

import Foundation

/// Which books a query is restricted to. Mirrors the server's `formats`
/// filter, plus the client-only "on this device" bucket.
struct LibraryFilter: Equatable, Sendable {
    var formats: [String] = []
    var downloadedOnly = false
    /// The viewer's hidden-formats preference (`UserSummary.hiddenFormats`).
    /// A book is excluded only when every format it carries is hidden; a
    /// formats-less row (physical-only) is never hidden — mirroring the
    /// server's predicate.
    var hiddenFormats: [String] = []

    static let none = LibraryFilter()
}

actor LibraryIndex {
    static let shared = LibraryIndex()

    /// How many books to pull per request while mirroring. Larger than the
    /// grid's page size — this is one background pass, not something a scroll
    /// is waiting on.
    private static let syncPageSize = 200

    /// Hard stop on one pass. A cursor that never advances would otherwise page
    /// forever against the server; nothing observed does that, which is exactly
    /// why it should be bounded rather than trusted.
    private static let maxSyncedBooks = 50_000

    /// Don't re-mirror more often than this. Foreground fires on every return
    /// to the app, including the one two seconds after leaving it.
    private static let minSyncInterval: TimeInterval = 300

    /// How long a half-finished pass may be resumed before it is restarted.
    ///
    /// Resuming spends fewer requests, but a keyset pass assumes the collection
    /// it is walking hasn't moved underneath it. Over an afternoon that stops
    /// being true — books get added, retitled, reindexed — and the resumed
    /// halves would no longer describe one library. An hour is long enough to
    /// survive the interruptions that actually happen (a tunnel, a locked
    /// phone) and short enough that the assumption still holds.
    private static let resumeWindow: TimeInterval = 3600

    /// Meta keys for a pass that outlives the process.
    private enum Key {
        static let cursor = "library_sync_cursor"
        static let startedAt = "library_sync_started_at"
        static let completedAt = "library_sync_completed_at"
    }

    private var syncing = false

    /// Whether the index has anything in it. Callers fall back to the network
    /// when it doesn't — a cold first launch has no mirror yet.
    func isPopulated() async -> Bool {
        await OfflineStore.shared.bookCount() > 0
    }

    /// Pull the library down and swap it in.
    ///
    /// A pass accumulates in `books_staging`, one committed page at a time, and
    /// only becomes the mirror once it finishes. Interrupting it costs nothing:
    /// the fetched pages stay staged and the next pass resumes from the stored
    /// cursor rather than starting over.
    ///
    /// Safe to call on every foreground — it no-ops while one pass is running
    /// and while the last completed one is recent. `force` is for the
    /// pull-to-refresh path, where the reader has explicitly asked.
    func sync(force: Bool = false) async {
        guard !syncing else { return }
        if !force, await recentlySynced() { return }
        syncing = true
        defer { syncing = false }

        var cursor = await resumeCursor()
        if cursor == nil { await beginPass() }
        var fetched = await OfflineStore.shared.stagedCount()
        var exhausted = false

        while fetched < Self.maxSyncedBooks {
            guard let page = await fetchPage(cursor: cursor) else {
                // Unreachable, or the server gave up part-way. Keep what this
                // pass has fetched — it resumes from `cursor` next time.
                await handleInterruption()
                return
            }

            await OfflineStore.shared.appendStagedBooks(Self.rows(for: page.value.books))
            fetched += page.value.books.count

            guard let next = page.nextCursor else {
                exhausted = true
                break
            }
            cursor = next
            await OfflineStore.shared.metaPut(Key.cursor, next)
        }

        // Stopping at the ceiling is not the same as reaching the end of the
        // library. Promoting there would publish a truncated pass as a complete
        // one, and the swap treats absence as deletion — so every book past the
        // bound would be dropped from the mirror on each sync.
        guard exhausted else {
            await handleInterruption()
            return
        }

        await OfflineStore.shared.promoteCompletedStaging()
        await clearPass()
        await OfflineStore.shared.metaPut(
            Key.completedAt, String(Int64(Date().timeIntervalSince1970))
        )
        await pruneOrphanedDownloads()
    }

    /// Drop downloads for books the library no longer has.
    ///
    /// A completed pass is the only moment this device knows what the library
    /// actually contains, and a book deleted on the server otherwise left its
    /// file on disk forever — gone from the "Downloaded" filter, which reads
    /// through `books`, while still counted under "Storage used" and
    /// undeletable from any screen.
    ///
    /// Guarded on a non-empty mirror: a server that answers with nothing is
    /// indistinguishable here from one whose library is genuinely empty, and
    /// wiping every downloaded book on the strength of that is not a trade
    /// worth making.
    ///
    /// Guarded per *format* for the same reason one step down. `GET
    /// /api/ebooks` lists the books under whichever library paths the server's
    /// settings currently name, so an audiobook path that is unset — or empty
    /// for as long as a reindex takes — drops every audiobook out of an
    /// otherwise complete pass. The non-empty check still passes on the
    /// strength of the ebooks, and pruning on that deleted every downloaded
    /// audiobook file on the device.
    private func pruneOrphanedDownloads() async {
        guard await OfflineStore.shared.bookCount() > 0 else { return }

        var kinds: Set<DownloadKind> = []
        for kind in DownloadKind.allCases {
            if await OfflineStore.shared.libraryHasKind(kind) { kinds.insert(kind) }
        }
        guard !kinds.isEmpty else { return }

        let removed = await OfflineStore.shared.pruneDownloadsMissingFromLibrary(kinds: kinds)
        guard !removed.isEmpty else { return }
        await DownloadManager.shared.forget(removed)
    }

    /// Whether a completed pass is recent enough to skip another.
    ///
    /// Stored rather than held in memory: an in-process timestamp resets on
    /// every launch, so the throttle never applied to the one sync that is
    /// guaranteed to happen — and quitting and reopening the app re-paged the
    /// entire library each time.
    private func recentlySynced() async -> Bool {
        guard let raw = await OfflineStore.shared.metaGet(Key.completedAt),
              let completed = TimeInterval(raw)
        else { return false }
        let age = Date().timeIntervalSince1970 - completed
        // A clock that moved backwards shouldn't wedge the throttle on.
        return age >= 0 && age < Self.minSyncInterval
    }

    /// The cursor to resume from, or `nil` to start a fresh pass. A pass that
    /// has aged out of `resumeWindow` is discarded rather than stitched onto.
    private func resumeCursor() async -> String? {
        guard let cursor = await OfflineStore.shared.metaGet(Key.cursor) else { return nil }
        guard let startedRaw = await OfflineStore.shared.metaGet(Key.startedAt),
              let started = TimeInterval(startedRaw),
              Date().timeIntervalSince1970 - started < Self.resumeWindow
        else {
            await OfflineStore.shared.clearStaging()
            return nil
        }
        return cursor
    }

    private func beginPass() async {
        await OfflineStore.shared.clearStaging()
        await OfflineStore.shared.metaPut(
            Key.startedAt, String(Int64(Date().timeIntervalSince1970))
        )
    }

    private func clearPass() async {
        await OfflineStore.shared.metaDelete(Key.cursor)
        await OfflineStore.shared.metaDelete(Key.startedAt)
    }

    /// A pass that stopped early.
    ///
    /// With a mirror already in place, the old one stays — it describes one
    /// coherent library, and half of a newer one doesn't. With no mirror yet
    /// there is nothing to protect and no deletion question to get wrong, so
    /// publish what was fetched: on a first sync, most of the library beats
    /// none of it, which is the whole reason the mirror exists.
    private func handleInterruption() async {
        guard await OfflineStore.shared.bookCount() == 0,
              await OfflineStore.shared.stagedCount() > 0
        else { return }
        await OfflineStore.shared.publishPartialStaging()
    }

    private func fetchPage(
        cursor: String?
    ) async -> (value: EbookLibrary, nextCursor: String?)? {
        var query: [String: String?] = [
            "sort": SortKey.title.rawValue,
            "dir": SortDirection.asc.rawValue,
            "limit": String(Self.syncPageSize),
        ]
        if let cursor { query["cursor"] = cursor }
        return try? await APIClient.shared.getPaged("/api/ebooks", query: query)
    }

    private static func rows(for books: [Book]) -> [OfflineStore.BookRow] {
        let encoder = JSONEncoder()
        return books.compactMap { book in
            guard let payload = try? encoder.encode(book) else { return nil }
            return row(for: book, payload: payload)
        }
    }

    /// Flatten a `Book` into its indexable columns. `internal` so the
    /// format-dedupe invariant the hidden-formats predicate relies on is
    /// testable.
    static func row(for book: Book, payload: Data) -> OfflineStore.BookRow {
        let author = book.creators.map(\.name).joined(separator: ", ")
        // One haystack per book so a match is a single LIKE rather than a scan
        // across five columns. Subjects are in it because a reader looking for
        // "cyberpunk" is searching the library the same way they'd search a
        // shelf, and the server's palette endpoint matches tags too.
        let haystack = [
            book.displayTitle,
            author,
            book.series ?? "",
            book.subjects.joined(separator: " "),
            book.publisher ?? "",
        ]
        .joined(separator: " ")
        .lowercased()

        return OfflineStore.BookRow(
            uuid: book.uuid,
            title: book.displayTitle.lowercased(),
            author: author.lowercased(),
            series: (book.series ?? "").lowercased(),
            seriesIndex: Double(book.seriesIndex ?? "") ?? 0,
            addedAt: book.addedAt ?? "",
            modified: book.modified ?? book.addedAt ?? "",
            // Deduped: the hidden-formats predicate strips each hidden token
            // from the CSV once, so a duplicate would survive the strip and
            // read as "still visible".
            formats: Set(book.formats.map { $0.lowercased() }).sorted().joined(separator: ","),
            searchText: haystack,
            payload: payload
        )
    }

    // MARK: - Reads

    /// One page out of the local mirror, ordered to match the server's sorts as
    /// closely as a local table can.
    func page(
        sort: SortKey,
        direction: SortDirection,
        filter: LibraryFilter,
        limit: Int,
        offset: Int
    ) async -> LibraryPageResult {
        let (clause, bindings) = Self.predicate(for: filter)
        let payloads = await OfflineStore.shared.bookPayloads(
            whereClause: clause,
            orderClause: Self.order(sort: sort, direction: direction),
            bindings: bindings,
            limit: limit,
            offset: offset
        )
        let total = await OfflineStore.shared.bookMatchCount(
            whereClause: clause, bindings: bindings
        )
        // The receipt, computed the server's way: same filter with the
        // exclusion lifted, diffed. First page only, like the wire header.
        var hiddenCount: Int64?
        if offset == 0, !filter.hiddenFormats.isEmpty {
            var unhidden = filter
            unhidden.hiddenFormats = []
            let (allClause, allBindings) = Self.predicate(for: unhidden)
            let all = await OfflineStore.shared.bookMatchCount(
                whereClause: allClause, bindings: allBindings
            )
            hiddenCount = all - total
        }
        let books = Self.decode(payloads)
        // A short page is the end of the library; anything else can be paged
        // for. The cursor is an offset because keyset ordering is the server's
        // contract, not this table's.
        let next = books.count < limit ? nil : Self.cursor(offset: offset + limit)
        return LibraryPageResult(
            books: books, nextCursor: next, total: total, hiddenCount: hiddenCount
        )
    }

    /// One book out of the local mirror.
    ///
    /// What the detail screen falls back to when the server can't be reached
    /// and this book's own replica entry was never written — which is every
    /// book whose detail page has not been opened before. The mirror stores the
    /// same `Book` the detail endpoint returns, so nothing is lost by answering
    /// from here.
    func book(uuid: String) async -> Book? {
        guard let payload = await OfflineStore.shared.bookPayload(uuid: uuid) else { return nil }
        return try? JSONDecoder().decode(Book.self, from: payload)
    }

    /// Books whose title, author, series, subjects, or publisher contain
    /// `query`. Ordered by where the match landed — a title hit is what the
    /// reader meant far more often than a subject hit.
    func search(_ query: String, limit: Int = 50) async -> [Book] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return [] }
        let pattern = "%\(Self.escapeLike(needle))%"

        let payloads = await OfflineStore.shared.bookPayloads(
            whereClause: "search_text LIKE ? ESCAPE '\\'",
            orderClause: """
                CASE
                    WHEN title = ? THEN 0
                    WHEN title LIKE ? ESCAPE '\\' THEN 1
                    WHEN author LIKE ? ESCAPE '\\' THEN 2
                    WHEN series LIKE ? ESCAPE '\\' THEN 3
                    ELSE 4
                END,
                title
                """,
            bindings: [
                pattern, needle, "\(Self.escapeLike(needle))%", pattern, pattern,
            ],
            limit: limit,
            offset: 0
        )
        return Self.decode(payloads)
    }

    /// The synthetic cursor `page` hands back, so `LibraryService` can tell a
    /// local cursor from one the server minted and resume in the right place.
    static func cursor(offset: Int) -> String { "local:\(offset)" }

    static func offset(fromCursor cursor: String?) -> Int? {
        guard let cursor, cursor.hasPrefix("local:") else { return nil }
        return Int(cursor.dropFirst("local:".count))
    }

    // MARK: - Query building

    /// SQL fragment + bindings for a filter. Every produced fragment is built
    /// from a closed set of cases; the only user-controlled values are bound.
    /// `internal` (not private) so the hidden-formats visibility rule is
    /// testable as a pure function.
    static func predicate(for filter: LibraryFilter) -> (String, [String]) {
        var clauses: [String] = []
        var bindings: [String] = []

        if !filter.formats.isEmpty {
            // `formats` is comma-joined, so match against a comma-delimited
            // haystack — otherwise "epub" would also match a hypothetical
            // "epub3" sitting in the same list.
            let matches = filter.formats.map { _ in "(',' || formats || ',') LIKE ? ESCAPE '\\'" }
            clauses.append("(\(matches.joined(separator: " OR ")))")
            bindings += filter.formats.map { "%,\(escapeLike($0.lowercased())),%" }
        }

        if !filter.hiddenFormats.isEmpty {
            // Visible iff stripping every hidden token from the CSV leaves a
            // non-empty remainder — i.e. some format is NOT hidden — or the
            // row has no formats at all (physical-only, never hidden). The
            // row writer stores lowercase deduped tokens, so one REPLACE per
            // token strips it completely.
            var expr = "(',' || formats || ',')"
            for token in filter.hiddenFormats.sorted() {
                expr = "REPLACE(\(expr), ?, ',')"
                bindings.append(",\(token.lowercased()),")
            }
            clauses.append("(formats = '' OR TRIM(\(expr), ',') <> '')")
        }

        if filter.downloadedOnly {
            // Scoping this in SQL rather than over the loaded page is what
            // makes "Downloaded" mean the whole library instead of whichever
            // sixty books happened to be on screen.
            clauses.append(
                "uuid IN (SELECT book_uuid FROM downloads WHERE state = 'complete')"
            )
        }

        return (clauses.joined(separator: " AND "), bindings)
    }

    private static func order(sort: SortKey, direction: SortDirection) -> String {
        let dir = direction == .asc ? "ASC" : "DESC"
        switch sort {
        case .title: return "title \(dir)"
        case .author: return "author \(dir), title ASC"
        case .series: return "series \(dir), series_index ASC, title ASC"
        case .lastUpdated: return "modified \(dir), title ASC"
        case .newestAdded: return "added_at \(dir), title ASC"
        }
    }

    /// `LIKE` treats `%` and `_` as wildcards; a reader typing either means the
    /// literal character. Paired with `ESCAPE '\'` at every call site.
    private static func escapeLike(_ value: String) -> String {
        value
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "%", with: "\\%")
            .replacingOccurrences(of: "_", with: "\\_")
    }

    private static func decode(_ payloads: [Data]) -> [Book] {
        let decoder = JSONDecoder()
        return payloads.compactMap { try? decoder.decode(Book.self, from: $0) }
    }
}
