//  LibraryService.swift
//  Reads over the library, discovery, and search surfaces.
//
//  Port of `frontend/src/data/{books,authors,series,tags}.rs`. Every read goes
//  through the replica cache so a cold launch offline still paints.

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

struct LibraryPageResult: Sendable {
    var books: [Book]
    var nextCursor: String?
    var total: Int64?
}

enum LibraryService {
    static let pageSize = 60

    /// One keyset page of the library. The first page is cached so a cold
    /// offline launch still shows a full shelf.
    static func page(
        sort: SortKey,
        direction: SortDirection,
        formats: [String],
        cursor: String?
    ) async throws -> LibraryPageResult {
        var query: [String: String?] = [
            "sort": sort.rawValue,
            "dir": direction.rawValue,
            "limit": String(pageSize),
        ]
        if !formats.isEmpty { query["formats"] = formats.joined(separator: ",") }
        if let cursor { query["cursor"] = cursor }

        let signature = "\(sort.rawValue).\(direction.rawValue).\(formats.sorted().joined(separator: "-"))"
        let isFirstPage = cursor == nil

        do {
            let (library, next): (EbookLibrary, String?) =
                try await APIClient.shared.getPaged("/api/ebooks", query: query)
            let result = LibraryPageResult(books: library.books, nextCursor: next, total: library.total)
            if isFirstPage {
                await Cache.write(CacheKey.libraryPage(signature), CachedPage(books: library.books, total: library.total))
            }
            return result
        } catch let error as APIError where error.isRecoverableOffline {
            guard isFirstPage,
                  let cached: CachedPage = await Cache.cachedOnly(CacheKey.libraryPage(signature))
            else { throw error }
            return LibraryPageResult(books: cached.books, nextCursor: nil, total: cached.total)
        }
    }

    private struct CachedPage: Codable, Sendable {
        var books: [Book]
        var total: Int64?
    }

    static func book(uuid: String) async throws -> Book {
        try await Cache.readThrough(CacheKey.book(uuid)) {
            try await APIClient.shared.get("/api/ebooks/\(uuid)")
        }
    }

    static func authors() async throws -> [AuthorSummary] {
        try await Cache.readThrough(CacheKey.authors) {
            try await APIClient.shared.get("/api/authors")
        }
    }

    static func author(id: Int64) async throws -> AuthorDetail {
        try await Cache.networkFirst(CacheKey.author(id)) {
            try await APIClient.shared.get("/api/authors/\(id)")
        }
    }

    static func series() async throws -> [SeriesSummary] {
        try await Cache.readThrough(CacheKey.series) {
            try await APIClient.shared.get("/api/series")
        }
    }

    static func series(id: Int64) async throws -> SeriesDetail {
        try await Cache.networkFirst(CacheKey.seriesDetail(id)) {
            try await APIClient.shared.get("/api/series/\(id)")
        }
    }

    static func tags() async throws -> [TagWeight] {
        try await Cache.readThrough(CacheKey.tags) {
            try await APIClient.shared.get("/api/tags")
        }
    }

    /// Live search for the palette-style results screen. Never cached — a
    /// stale hit list is worse than an empty one.
    static func searchPalette(query: String) async throws -> PaletteResults {
        try await APIClient.shared.get("/api/search/palette", query: ["q": query])
    }

    static func searchFull(query: String) async throws -> EbookLibrary {
        try await APIClient.shared.get("/api/search", query: ["q": query])
    }

    static func suggestions(uuid: String) async throws -> [SuggestedBook] {
        try await APIClient.shared.get("/api/ebooks/\(uuid)/suggestions")
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
