//  Cache.swift
//  Read-through replica cache over `OfflineStore`.
//
//  Port of `frontend/src/offline/cache.rs`. Two policies: cache-first (serve
//  the replica instantly, refresh behind it) and network-first (used where a
//  stale answer would be actively wrong, e.g. `me` during an account switch).

import Foundation

enum CacheKey {
    static let library = "books"
    static let me = "me"
    static let recentProgress = "recent_progress"
    static let authors = "authors"
    static let series = "series"
    static let tags = "tags"
    static let shelves = "shelves"
    static let shelfPreviews = "shelf_previews"

    static func book(_ uuid: String) -> String { "book:\(uuid)" }
    static func author(_ id: Int64) -> String { "author:\(id)" }
    static func seriesDetail(_ id: Int64) -> String { "series:\(id)" }
    static func shelf(_ id: Int64) -> String { "shelf:\(id)" }
    static func shelfPage(_ id: Int64) -> String { "shelf_page:\(id)" }
    static func progress(_ uuid: String) -> String { "progress:\(uuid)" }
    static func highlights(_ uuid: String) -> String { "highlights:\(uuid)" }
    static func bookmarks(_ uuid: String) -> String { "bookmarks:\(uuid)" }
    static func journals(_ uuid: String) -> String { "journals:\(uuid)" }
    static func rating(_ uuid: String) -> String { "rating:\(uuid)" }
    static func ratingsOthers(_ uuid: String) -> String { "ratings_others:\(uuid)" }
    static func readStatus(_ uuid: String) -> String { "read_status:\(uuid)" }
    static func stats(_ range: StatsRange) -> String { "stats:\(range.rawValue)" }
    static func manifest(_ uuid: String) -> String { "manifest:\(uuid)" }
    static func libraryPage(_ signature: String) -> String { "books_page:\(signature)" }
}

enum Cache {
    private static let encoder = JSONEncoder()
    private static let decoder = JSONDecoder()

    static func read<T: Codable & Sendable>(_ key: String, as type: T.Type = T.self) async -> T? {
        guard let data = await OfflineStore.shared.cacheGet(key) else { return nil }
        return try? decoder.decode(T.self, from: data)
    }

    static func write<T: Codable & Sendable>(_ key: String, _ value: T) async {
        guard let data = try? encoder.encode(value) else { return }
        await OfflineStore.shared.cachePut(key, data)
    }

    /// How long a cached value may be served without a round trip. Past this
    /// the read still *falls back* to the replica when the network fails, but
    /// it no longer hands stale data to a device that is online.
    ///
    /// Without a bound, a value changed anywhere else — another device, the
    /// web client, a script — stayed invisible indefinitely: `readThrough`
    /// refreshed the replica in the background, but the caller already had the
    /// old value and nothing published the update.
    static let freshnessWindow: TimeInterval = 45

    /// Cache-first within `freshnessWindow`, network-first past it, replica
    /// fallback either way when offline.
    static func readThrough<T: Codable & Sendable>(
        _ key: String,
        fetch: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        let age = await OfflineStore.shared.cacheAge(key)

        if let age, age < freshnessWindow, let cached: T = await read(key) {
            Task.detached(priority: .utility) {
                guard let fresh = try? await fetch() else { return }
                await write(key, fresh)
            }
            return cached
        }

        do {
            let fresh = try await fetch()
            await write(key, fresh)
            return fresh
        } catch let error as APIError where error.isRecoverableOffline {
            if let cached: T = await read(key) { return cached }
            throw APIError.offline
        }
    }

    /// Network-first: try the server, fall back to the replica on a transport
    /// failure. Use where a stale answer is worse than a slow one.
    static func networkFirst<T: Codable & Sendable>(
        _ key: String,
        fetch: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        do {
            let fresh = try await fetch()
            await write(key, fresh)
            return fresh
        } catch let error as APIError where error.isRecoverableOffline {
            if let cached: T = await read(key) { return cached }
            throw error
        }
    }

    /// Serve whatever is cached without touching the network — the offline
    /// fallback used when a fetch has already failed.
    static func cachedOnly<T: Codable & Sendable>(_ key: String) async -> T? {
        await read(key)
    }
}
