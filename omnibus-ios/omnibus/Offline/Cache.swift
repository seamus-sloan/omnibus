//  Cache.swift
//  Local-first replica cache over `OfflineStore`.
//
//  Port of `frontend/src/offline/cache.rs`. Reads go through `live`: the
//  replica answers immediately and the server's answer follows if it differs,
//  so no screen ever waits on the network to paint.

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
    /// Scoped by format as well as book: a dual-format book has two positions
    /// and they are not interchangeable. Sharing one slot meant the reader and
    /// the player overwrote each other's place in the same book.
    static func progress(_ uuid: String, _ format: ProgressFormat) -> String {
        "progress:\(uuid):\(format.rawValue)"
    }
    static func highlights(_ uuid: String) -> String { "highlights:\(uuid)" }
    static func bookmarks(_ uuid: String) -> String { "bookmarks:\(uuid)" }
    static func journals(_ uuid: String) -> String { "journals:\(uuid)" }
    static func rating(_ uuid: String) -> String { "rating:\(uuid)" }
    static func ratingsOthers(_ uuid: String) -> String { "ratings_others:\(uuid)" }
    static func readStatus(_ uuid: String) -> String { "read_status:\(uuid)" }
    static func stats(_ range: StatsRange) -> String { "stats:\(range.rawValue)" }
    static func manifest(_ uuid: String) -> String { "manifest:\(uuid)" }
    static func libraryPage(_ signature: String) -> String { "books_page:\(signature)" }
    static func playbackRate(_ uuid: String) -> String { "audio_rate:\(uuid)" }
    static func suggestions(_ uuid: String) -> String { "suggestions:\(uuid)" }
    static func shelvesContaining(_ uuid: String) -> String { "shelves_with:\(uuid)" }
}

/// Which replica keys a queued write would make the server's answer wrong for.
///
/// Revalidation used to be gated on the outbox's total depth, so a single
/// pending op of any kind held every read in the app on the replica. That is
/// far more than the invariant needs — a queued session report cannot make the
/// server's shelf list stale — and it turned one op that could not land into an
/// app-wide freeze on fresh data. This narrows the hold to the keys the pending
/// writes actually describe.
enum OutboxScope {
    /// Whether any of `kinds` describes a write the server has not seen that
    /// would make its answer for `key` older than what this device holds.
    static func isBlocked(_ key: String, by kinds: Set<String>) -> Bool {
        kinds.contains { touches(key, kind: $0) }
    }

    private static func touches(_ key: String, kind: String) -> Bool {
        switch kind {
        case OpKind.highlight: return key.hasPrefix("highlights:")
        case OpKind.bookmark: return key.hasPrefix("bookmarks:")
        // Journal entries feed the completion side of the stats summary.
        case OpKind.journal: return key.hasPrefix("journals:") || key.hasPrefix("stats:")
        case OpKind.session: return key.hasPrefix("stats:")
        case OpKind.shelfMembership:
            return key == CacheKey.shelves
                || key == CacheKey.shelfPreviews
                || key.hasPrefix("shelf:")
                || key.hasPrefix("shelf_page:")
                || key.hasPrefix("shelves_with:")
        default: break
        }

        // The rest are book-scoped and carry their uuid — and, for a position,
        // its format — in the kind itself, so each names exactly one key.
        if let uuid = suffix(of: kind, after: "playback_rate:") {
            return key == CacheKey.playbackRate(uuid)
        }
        if let uuid = suffix(of: kind, after: "rating:") {
            return key == CacheKey.rating(uuid)
        }
        if let uuid = suffix(of: kind, after: "read_status:") {
            // Marking a book finished is one of the two events the stats
            // summary counts as a completion (`FINISHED_EVENTS` in
            // `db/src/stats/mod.rs`) — the other is a 100% journal entry,
            // which has always declared it. Leaving this one undeclared let
            // the stats screen revalidate against a server that had not seen
            // the change and read back the pre-change count.
            return key == CacheKey.readStatus(uuid) || key.hasPrefix("stats:")
        }
        if kind.hasPrefix("progress:") {
            // The kind is the progress key verbatim, and any position write
            // also changes what the Continue rail shows.
            return key == kind || key == CacheKey.recentProgress
        }

        // An unrecognised kind blocks everything. A write path that grows a new
        // kind without declaring its scope here should be over-cautious rather
        // than quietly let a stale answer overwrite what it queued.
        return true
    }

    private static func suffix(of kind: String, after prefix: String) -> String? {
        guard kind.hasPrefix(prefix) else { return nil }
        return String(kind.dropFirst(prefix.count))
    }
}

/// One answer from a `Cache.live` read.
struct CacheRead<T: Sendable>: Sendable {
    let value: T
    /// False while this is the replica's answer and the server has not
    /// confirmed it. Callers that must not act on a provisional value — the
    /// reader deciding where to open — branch on this.
    let isFresh: Bool
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

    /// A read that answers twice: the replica now, the server's answer when it
    /// lands and only if it actually differs.
    ///
    /// This is what makes the app local-first. The previous policies each
    /// returned once, so anything volatile had to be network-first — which
    /// meant every screen paid a full round trip before it could paint, and a
    /// server that was reachable-but-dead cost the entire request timeout per
    /// read. Here nothing waits: the caller draws the replica immediately and
    /// the view updates underneath it if the server disagrees.
    ///
    /// The stream finishing means the read settled. It throws only when there
    /// was no replica to fall back on — once a value has gone out, an offline
    /// failure ends the stream quietly rather than replacing good data with an
    /// error.
    static func live<T: Codable & Sendable>(
        _ key: String,
        fetch: @escaping @Sendable () async throws -> T
    ) -> AsyncThrowingStream<CacheRead<T>, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                let cachedData = await OfflineStore.shared.cacheGet(key)
                var delivered = false
                if let cachedData, let value = try? decoder.decode(T.self, from: cachedData) {
                    continuation.yield(CacheRead(value: value, isFresh: false))
                    delivered = true
                }

                guard await revalidationIsSafe(for: key) else {
                    // Finishing empty here is what let an invalidated key read
                    // as "there is nothing" rather than "this can't be
                    // answered": deleting a shelf offline wiped the shelves
                    // cache, and the next read then rendered the no-shelves
                    // empty state. With no replica to fall back on, the honest
                    // answer is a failure the caller can show.
                    continuation.finish(throwing: delivered ? nil : APIError.offline)
                    return
                }

                do {
                    let fresh = try await fetch()

                    // Re-checked *after* the fetch as well as before it. The
                    // guard above proves the queue was clear when the request
                    // went out, which says nothing about a write made while it
                    // was in flight — and a request is a long time on the
                    // connections where this matters. Adopting the answer then
                    // overwrites the optimistic row that write just left,
                    // which is the exact failure the pre-fetch guard exists to
                    // prevent, arrived at through a window one request wide.
                    if await isBlocked(key) {
                        await yieldReplica(key, since: cachedData, to: continuation)
                        continuation.finish()
                        return
                    }

                    // Cached before the cancellation check, not after. SwiftUI
                    // tears these streams down on every navigation, so an
                    // answer that lands as the view leaves is common — and
                    // discarding it threw away data the app had already paid to
                    // fetch, leaving the replica stale for the next screen that
                    // asks. Nothing is yielded to a consumer that has gone; the
                    // replica is written either way.
                    let freshData = try? encoder.encode(fresh)
                    if let freshData { await OfflineStore.shared.cachePut(key, freshData) }
                    guard !Task.isCancelled else {
                        continuation.finish()
                        return
                    }
                    // Byte-compare rather than `Equatable`: the store holds
                    // `Data` on both sides already, and re-yielding an
                    // identical answer would churn every observing view for
                    // nothing.
                    if freshData == nil || freshData != cachedData {
                        continuation.yield(CacheRead(value: fresh, isFresh: true))
                    }
                    continuation.finish()
                } catch {
                    // A cancelled read is the consumer going away, not a
                    // failure worth reporting to it.
                    guard !isCancellation(error) else {
                        continuation.finish()
                        return
                    }
                    // Only an offline failure is absorbed, and only once the
                    // replica has gone out. A 401 or a decode failure still
                    // propagates — swallowing those would leave a caller
                    // treating an expired session as a successful read.
                    let offline = (error as? APIError)?.isRecoverableOffline ?? false
                    continuation.finish(throwing: delivered && offline ? nil : error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Whether the server's answer can be trusted to replace the replica.
    ///
    /// It can't while writes are still queued: the server has not seen them, so
    /// its answer describes a state this device has already moved past, and the
    /// `cachePut` below would make that stale answer the replica — erasing the
    /// optimistic row the outbox is still carrying. A highlight made offline
    /// disappeared exactly this way, the moment anything read the list back.
    ///
    /// Draining first is what usually makes the answer safe rather than merely
    /// avoided; only a drain that couldn't finish holds the read off.
    ///
    /// Scoped to the keys the queue actually describes, via [`OutboxScope`] —
    /// gating on the queue's depth alone meant one stuck op froze every read in
    /// the app, not just the rows it was about.
    private static func revalidationIsSafe(for key: String) async -> Bool {
        guard await isBlocked(key) else { return true }
        await SyncEngine.shared.drain()
        return !(await isBlocked(key))
    }

    /// Whether the queue currently holds a write the server's answer for `key`
    /// would predate. Deliberately does not drain — the post-fetch check runs
    /// inside a read, and draining there would push writes from a code path
    /// whose whole job is to pull.
    static func isBlocked(_ key: String) async -> Bool {
        OutboxScope.isBlocked(key, by: await OfflineStore.shared.pendingKinds())
    }

    /// Hand back whatever the replica holds *now*, when the server's answer had
    /// to be discarded. The write that arrived mid-fetch has already updated
    /// it, so this is the newest truth the device has — and a caller that
    /// never got a first value (nothing was cached when the read began) would
    /// otherwise be left with nothing at all.
    private static func yieldReplica<T: Codable & Sendable>(
        _ key: String,
        since delivered: Data?,
        to continuation: AsyncThrowingStream<CacheRead<T>, Error>.Continuation
    ) async {
        guard !Task.isCancelled,
              let current = await OfflineStore.shared.cacheGet(key),
              current != delivered,
              let value = try? decoder.decode(T.self, from: current)
        else { return }
        continuation.yield(CacheRead(value: value, isFresh: false))
    }

    /// Collapse a `live` read to a single settled value. For callers with
    /// nowhere to put an intermediate answer — a share sheet, a one-shot
    /// export — never for anything that paints.
    static func settled<T: Codable & Sendable>(
        _ key: String,
        fetch: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        var latest: T?
        for try await read in live(key, fetch: fetch) { latest = read.value }
        guard let latest else { throw APIError.offline }
        return latest
    }

    /// Apply a change to a cached value in place. A no-op when nothing is
    /// cached — the next read fetches the real thing anyway.
    static func mutate<T: Codable & Sendable>(_ key: String, _ change: (inout T) -> Void) async {
        guard var value: T = await read(key) else { return }
        change(&value)
        await write(key, value)
    }

    /// Serve whatever is cached without touching the network.
    static func cachedOnly<T: Codable & Sendable>(_ key: String) async -> T? {
        await read(key)
    }
}

extension AsyncThrowingStream {
    /// The values from a `live` read with the phase and the failure dropped —
    /// for views that render whatever is newest and have nothing useful to say
    /// about an error. A failed read simply yields nothing, leaving whatever
    /// the view already had on screen.
    func values<T>() -> AsyncStream<T> where Element == CacheRead<T>, Failure == Error {
        AsyncStream { continuation in
            let task = Task {
                do {
                    for try await read in self { continuation.yield(read.value) }
                } catch {}
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
