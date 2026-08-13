//  LibraryService.swift
//  Reads over the library, discovery, and search surfaces.
//
//  Port of `frontend/src/data/{books,authors,series,tags}.rs`. Every read is a
//  `Cache.live` stream: the replica paints immediately and the server's answer
//  follows if it differs, so a cold launch offline is indistinguishable from a
//  warm one until something actually needs the network.

import Foundation

enum SortKey: String, CaseIterable, Codable, Sendable {
    case title
    case author
    case series
    case lastUpdated = "last_updated"
    case newestAdded = "newest_added"

    var label: String {
        switch self {
        case .title: "Title"
        case .author: "Author"
        case .series: "Series"
        case .lastUpdated: "Last updated"
        case .newestAdded: "Newest added"
        }
    }
}

enum SortDirection: String, Codable, Sendable {
    case asc, desc

    var toggled: SortDirection { self == .asc ? .desc : .asc }
}

/// `Codable` because the first page is cached verbatim — the cursor included,
/// so a replayed page can still scroll forward before the server answers.
struct LibraryPageResult: Codable, Sendable {
    var books: [Book]
    var nextCursor: String?
    var total: Int64?
    /// The "N hidden" receipt: how many books the hidden-formats exclusion
    /// removed. First page only; `nil` when nothing is hidden. Optional keeps
    /// pre-upgrade cached pages decoding.
    var hiddenCount: Int64?
}

enum LibraryService {
    static let pageSize = 60

    /// One keyset page of the library.
    ///
    /// The server is authoritative whenever it answers — it owns the sort
    /// collation and the keyset cursors. When it doesn't, this falls through to
    /// [`LibraryIndex`], the local mirror of the whole library, so scrolling
    /// past the first page offline reaches the rest of the shelf instead of a
    /// wall. A cursor the index minted (`local:<offset>`) is served from the
    /// index without trying the network at all — the server has never seen it.
    static func page(
        sort: SortKey,
        direction: SortDirection,
        filter: LibraryFilter,
        cursor: String?
    ) -> AsyncThrowingStream<CacheRead<LibraryPageResult>, Error> {
        let query = pageQuery(
            sort: sort, direction: direction, formats: filter.formats,
            excludeFormats: filter.hiddenFormats, cursor: cursor
        )
        let signature = pageSignature(sort: sort, direction: direction, filter: filter)

        // A cursor the index minted, or a bucket the server can't express:
        // answered locally, no network attempted.
        if let offset = LibraryIndex.offset(fromCursor: cursor) {
            return localPage(sort: sort, direction: direction, filter: filter, offset: offset)
        }
        if filter.downloadedOnly {
            return localPage(sort: sort, direction: direction, filter: filter, offset: 0)
        }

        // Later pages are a scroll-position artefact, not state worth caching —
        // one shot at the server, then the mirror.
        guard cursor == nil else {
            return remotePage(query: query) {
                await LibraryIndex.shared.page(
                    sort: sort, direction: direction, filter: filter,
                    limit: pageSize, offset: 0
                )
            }
        }

        return firstPage(query: query, signature: signature) {
            await LibraryIndex.shared.page(
                sort: sort, direction: direction, filter: filter, limit: pageSize, offset: 0
            )
        }
    }

    /// A page straight out of the local mirror.
    private static func localPage(
        sort: SortKey, direction: SortDirection, filter: LibraryFilter, offset: Int
    ) -> AsyncThrowingStream<CacheRead<LibraryPageResult>, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                let page = await LibraryIndex.shared.page(
                    sort: sort, direction: direction, filter: filter,
                    limit: pageSize, offset: offset
                )
                continuation.yield(CacheRead(value: page, isFresh: false))
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// One shot at the server, falling back to the mirror rather than stranding
    /// the grid mid-scroll. The failed cursor was the server's, so the mirror
    /// can't resume from it — it restarts at the top, which still beats a wall.
    private static func remotePage(
        query: [String: String?],
        fallback: @escaping @Sendable () async -> LibraryPageResult
    ) -> AsyncThrowingStream<CacheRead<LibraryPageResult>, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (library, next, hidden): (EbookLibrary, String?, Int64?) =
                        try await APIClient.shared.getPagedCounted("/api/ebooks", query: query)
                    continuation.yield(
                        CacheRead(
                            value: LibraryPageResult(
                                books: library.books, nextCursor: next,
                                total: library.total, hiddenCount: hidden
                            ),
                            isFresh: true
                        )
                    )
                    continuation.finish()
                } catch {
                    let page = await fallback()
                    if page.books.isEmpty {
                        continuation.finish(throwing: error)
                    } else {
                        continuation.yield(CacheRead(value: page, isFresh: false))
                        continuation.finish()
                    }
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// The first page: replica, then server, then — if the server can't be
    /// reached and nothing was cached — the mirror. The last hop is what makes
    /// a cold launch offline show a library instead of an error.
    private static func firstPage(
        query: [String: String?],
        signature: String,
        fallback: @escaping @Sendable () async -> LibraryPageResult
    ) -> AsyncThrowingStream<CacheRead<LibraryPageResult>, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                var delivered = false
                do {
                    let live = Cache.live(CacheKey.libraryPage(signature)) {
                        let (library, next, hidden): (EbookLibrary, String?, Int64?) =
                            try await APIClient.shared.getPagedCounted("/api/ebooks", query: query)
                        return LibraryPageResult(
                            books: library.books, nextCursor: next,
                            total: library.total, hiddenCount: hidden
                        )
                    }
                    for try await read in live {
                        delivered = true
                        continuation.yield(read)
                    }
                    continuation.finish()
                } catch {
                    guard !delivered else {
                        continuation.finish()
                        return
                    }
                    let page = await fallback()
                    if page.books.isEmpty {
                        continuation.finish(throwing: error)
                    } else {
                        continuation.yield(CacheRead(value: page, isFresh: false))
                        continuation.finish()
                    }
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func pageQuery(
        sort: SortKey, direction: SortDirection, formats: [String],
        excludeFormats: [String], cursor: String?
    ) -> [String: String?] {
        var query: [String: String?] = [
            "sort": sort.rawValue,
            "dir": direction.rawValue,
            "limit": String(pageSize),
        ]
        if !formats.isEmpty { query["formats"] = formats.joined(separator: ",") }
        if !excludeFormats.isEmpty {
            query["exclude_formats"] = excludeFormats.sorted().joined(separator: ",")
        }
        if let cursor { query["cursor"] = cursor }
        return query
    }

    static func pageSignature(
        sort: SortKey, direction: SortDirection, filter: LibraryFilter
    ) -> String {
        let formats = filter.formats.sorted().joined(separator: "-")
        // The hidden-formats pref keys the cached first page: a pref change
        // must miss the cache or a stale page serves right after a toggle.
        let hidden = filter.hiddenFormats.sorted().joined(separator: "-")
        return "\(sort.rawValue).\(direction.rawValue).\(formats)|hide:\(hidden)"
    }

    /// One book: replica, then the server, then the local mirror.
    ///
    /// The mirror hop is what makes a book openable offline that this device
    /// has never opened before. The replica only holds what a screen has
    /// already asked for, so without it the detail page of any unvisited book
    /// answered "Could not connect to the server" while the whole `Book` sat in
    /// the mirror a table away — the library grid has fallen back this way
    /// since it was written, and the detail page simply never did.
    static func book(uuid: String) -> AsyncThrowingStream<CacheRead<Book>, Error> {
        withMirrorFallback(
            Cache.live(CacheKey.book(uuid)) {
                try await APIClient.shared.get("/api/ebooks/\(uuid)")
            }
        ) {
            await LibraryIndex.shared.book(uuid: uuid)
        }
    }

    /// Back a live read with the local mirror, used only when the read failed
    /// having produced nothing.
    private static func withMirrorFallback<T: Codable & Sendable>(
        _ live: AsyncThrowingStream<CacheRead<T>, Error>,
        _ fallback: @escaping @Sendable () async -> T?
    ) -> AsyncThrowingStream<CacheRead<T>, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                var delivered = false
                do {
                    for try await read in live {
                        delivered = true
                        continuation.yield(read)
                    }
                    continuation.finish()
                } catch {
                    // Something was already on screen, or the mirror has
                    // nothing either — in both cases the error is the read's to
                    // report, not the mirror's to paper over.
                    guard !delivered, let value = await fallback() else {
                        continuation.finish(throwing: delivered ? nil : error)
                        return
                    }
                    continuation.yield(CacheRead(value: value, isFresh: false))
                    continuation.finish()
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static func authors() -> AsyncThrowingStream<CacheRead<[AuthorSummary]>, Error> {
        Cache.live(CacheKey.authors) {
            try await APIClient.shared.get("/api/authors")
        }
    }

    static func author(id: Int64) -> AsyncThrowingStream<CacheRead<AuthorDetail>, Error> {
        Cache.live(CacheKey.author(id)) {
            try await APIClient.shared.get("/api/authors/\(id)")
        }
    }

    static func series() -> AsyncThrowingStream<CacheRead<[SeriesSummary]>, Error> {
        Cache.live(CacheKey.series) {
            try await APIClient.shared.get("/api/series")
        }
    }

    static func series(id: Int64) -> AsyncThrowingStream<CacheRead<SeriesDetail>, Error> {
        Cache.live(CacheKey.seriesDetail(id)) {
            try await APIClient.shared.get("/api/series/\(id)")
        }
    }

    static func tags() -> AsyncThrowingStream<CacheRead<[TagWeight]>, Error> {
        Cache.live(CacheKey.tags) {
            try await APIClient.shared.get("/api/tags")
        }
    }

    static func genres() -> AsyncThrowingStream<CacheRead<[GenreWeight]>, Error> {
        Cache.live(CacheKey.genres) {
            try await APIClient.shared.get("/api/genres")
        }
    }

    /// One settled `Book`, for callers with nowhere to put a provisional one.
    static func settledBook(uuid: String) async throws -> Book {
        try await Cache.settled(CacheKey.book(uuid)) {
            try await APIClient.shared.get("/api/ebooks/\(uuid)")
        }
    }

    /// Which files make up an audiobook and how long each one runs.
    ///
    /// Settled rather than live: the manifest decides which URLs the player is
    /// built against, so swapping it underneath a prepared `AVPlayer` would
    /// mean rebuilding the queue mid-playback.
    ///
    /// Shared with the download path, which needs it cached *before* the
    /// device goes offline — the audio file alone doesn't say how to lay it
    /// out on a timeline, so a downloaded audiobook whose manifest had never
    /// been fetched could not be opened without a network.
    /// `fileID` targets one `book_files` row of a multi-file book; `nil` is
    /// the server's default (first audio file by ordinal). The server 404s an
    /// id that no longer names a file of this book — `book_files` rows churn
    /// on reindex — and the caller decides whether to fall back.
    static func audiobookManifest(uuid: String, fileID: Int64? = nil) async throws
        -> AudiobookManifest
    {
        var query: [String: String?] = [:]
        if let fileID { query["file_id"] = String(fileID) }
        return try await Cache.settled(CacheKey.manifest(uuid, fileID: fileID)) {
            try await APIClient.shared.get("/api/audiobooks/\(uuid)/manifest", query: query)
        }
    }

    /// How long the server leg of a search waits before firing, so a fast
    /// typist doesn't spend a rate-limited request per keystroke. The local leg
    /// isn't debounced — it reads a table the device already holds.
    private static let searchDebounce: Duration = .milliseconds(220)

    /// Grouped search for the results screen — local first, then the server.
    ///
    /// The stream answers twice, the same shape every other read uses: the
    /// local mirror matches immediately, and the server's richer answer (which
    /// knows authors, series, and tags as entities, and ranks by BM25) replaces
    /// it when it lands. Searching used to be the one surface that always
    /// waited on a round trip and returned nothing at all offline — on a
    /// library the device already holds, that was the wrong answer to a
    /// question it could answer itself.
    ///
    /// Cancelling the consuming task cancels the debounce with it, which is
    /// what makes this safe to restart on every keystroke.
    static func searchPalette(query: String) -> AsyncStream<PaletteResults> {
        search(query: query, localLimit: 8) {
            try await APIClient.shared.get("/api/search/palette", query: ["q": query])
        } localAnswer: {
            PaletteResults(localBooks: $0)
        } emptyAnswer: {
            PaletteResults()
        }
    }

    /// Full result set for one query, local first then the server's ranking.
    static func searchFull(query: String) -> AsyncStream<[Book]> {
        search(query: query, localLimit: 200) {
            let library: EbookLibrary = try await APIClient.shared.get(
                "/api/search", query: ["q": query]
            )
            return library.books
        } localAnswer: {
            $0
        } emptyAnswer: {
            []
        }
    }

    /// Shared two-phase search: mirror now, server after the debounce.
    private static func search<T: Sendable>(
        query: String,
        localLimit: Int,
        remote: @escaping @Sendable () async throws -> T,
        localAnswer: @escaping @Sendable ([Book]) -> T,
        emptyAnswer: @escaping @Sendable () -> T
    ) -> AsyncStream<T> {
        AsyncStream { continuation in
            let task = Task {
                let local = await LibraryIndex.shared.search(query, limit: localLimit)
                guard !Task.isCancelled else { return continuation.finish() }
                if !local.isEmpty { continuation.yield(localAnswer(local)) }

                try? await Task.sleep(for: searchDebounce)
                guard !Task.isCancelled else { return continuation.finish() }

                if let answer = try? await remote(), !Task.isCancelled {
                    continuation.yield(answer)
                } else if local.isEmpty, !Task.isCancelled {
                    // Nothing locally and no server: an empty result is the
                    // honest answer, and the caller needs one to stop loading.
                    continuation.yield(emptyAnswer())
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// "Readers also enjoyed" for a book. Cached because it's an upstream
    /// Hardcover call behind the server — slow enough that re-fetching it on
    /// every visit to a book is felt, and stable enough that a remembered
    /// answer is a good one.
    static func suggestions(uuid: String) -> AsyncThrowingStream<CacheRead<[SuggestedBook]>, Error> {
        Cache.live(CacheKey.suggestions(uuid)) {
            try await APIClient.shared.get("/api/ebooks/\(uuid)/suggestions")
        }
    }

    // MARK: - Covers

    static func coverURL(for book: Book, size: ThumbSize = .md) async -> URL? {
        guard book.coverURL != nil else { return nil }
        return await APIClient.shared.absoluteURL("/api/thumbs/\(book.uuid)/\(size.rawValue)")
    }

    static func coverURL(uuid: String, size: ThumbSize = .md) async -> URL? {
        await APIClient.shared.absoluteURL("/api/thumbs/\(uuid)/\(size.rawValue)")
    }

    static func fullCoverURL(uuid: String) async -> URL? {
        await APIClient.shared.absoluteURL("/api/covers/\(uuid)")
    }
}

/// Mirrors `db::thumbs::ThumbSize` — the server serves exactly these three and
/// 404s anything else.
enum ThumbSize: String, Sendable {
    case sm, md, lg
}

struct SuggestedBook: Codable, Hashable, Sendable, Identifiable {
    var rank: Int
    var title: String
    var author: String?
    var reason: String?

    var id: Int { rank }
}
