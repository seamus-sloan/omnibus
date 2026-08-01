//  ComicPages.swift
//  The pager's page source: one actor, two backings.
//
//  Online, pages come one at a time from `GET /api/ebooks/{uuid}/pages/{n}`
//  — the server extracts entries so the client never downloads the archive
//  to read. Offline, the downloaded CBZ answers through `ComicArchive`, in
//  the same page order, so a saved page index means the same thing on both.

import Foundation

actor ComicPages {
    /// Recently-read pages held for instant back-and-forth turns. Small on
    /// purpose — scanned pages run megabytes each, and the pager only ever
    /// needs the neighbourhood of the current page.
    private static let cacheLimit = 8

    let count: Int

    private enum Backing {
        case remote(uuid: String)
        case local(ComicArchive)
    }

    private let backing: Backing
    private var cache: [Int: Data] = [:]
    /// Insertion order for eviction, oldest first.
    private var order: [Int] = []
    /// One fetch per page however many views ask — a turn plus a prefetch
    /// land on the same task rather than the same request twice.
    private var inflight: [Int: Task<Data, Error>] = [:]

    /// A source over the server's per-page endpoint. `count` comes from the
    /// detail payload's `page_count`.
    init(uuid: String, count: Int) {
        backing = .remote(uuid: uuid)
        self.count = count
    }

    /// A source over a downloaded archive. The archive itself is the page
    /// count, so a shrunken re-download can't leave the pager aiming past
    /// the end.
    init(localURL: URL) throws {
        let archive = try ComicArchive(url: localURL)
        backing = .local(archive)
        count = archive.pages.count
    }

    /// The bytes of page `index`, from cache, an in-flight fetch, or the
    /// backing.
    func page(_ index: Int) async throws -> Data {
        if let cached = cache[index] { return cached }
        if let running = inflight[index] { return try await running.value }

        let task = Task { try await self.read(index) }
        inflight[index] = task
        defer { inflight[index] = nil }
        let bytes = try await task.value
        store(bytes, at: index)
        return bytes
    }

    /// Warm the neighbours of `index` — the n±1 prefetch that makes a page
    /// turn instant. Failures are dropped; the page is fetched again with a
    /// real error path when it's actually turned to.
    func prefetch(around index: Int) {
        for neighbour in [index + 1, index - 1]
        where (0..<count).contains(neighbour) && cache[neighbour] == nil
            && inflight[neighbour] == nil
        {
            let task = Task { try await self.read(neighbour) }
            inflight[neighbour] = task
            Task {
                defer { self.inflight[neighbour] = nil }
                guard let bytes = try? await task.value else { return }
                self.store(bytes, at: neighbour)
            }
        }
    }

    private func store(_ bytes: Data, at index: Int) {
        if cache[index] == nil { order.append(index) }
        cache[index] = bytes
        while order.count > Self.cacheLimit {
            let evicted = order.removeFirst()
            cache[evicted] = nil
        }
    }

    /// One page read against the backing. Actor-isolated on purpose: local
    /// extracts share one `Archive` (a seeking file handle that must never
    /// see two readers at once), while a remote read suspends on the network
    /// and leaves the actor free.
    private func read(_ index: Int) async throws -> Data {
        switch backing {
        case let .remote(uuid):
            return try await APIClient.shared.data(for: "/api/ebooks/\(uuid)/pages/\(index)")
        case let .local(archive):
            return try archive.page(at: index)
        }
    }
}
