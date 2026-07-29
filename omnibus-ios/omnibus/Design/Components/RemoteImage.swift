//  RemoteImage.swift
//  Authenticated image loading with a memory + disk cache.
//
//  `AsyncImage` can't attach the bearer header the cover endpoints require,
//  and covers are the single most-repeated request in the app, so they get a
//  real two-tier cache. The disk tier doubles as the offline cover store.

import CryptoKit
import SwiftUI
import UIKit

actor ImageCache {
    static let shared = ImageCache()

    /// How stale a cached image may be before the cache checks back with the
    /// server. Covers change rarely and the check is a background 304, but
    /// without a window every cell scrolling into view would fire one
    /// conditional request. Five minutes keeps a cover edited on another
    /// device arriving promptly while leaving scrolling free.
    private static let revalidateAfter: TimeInterval = 300

    private let memory = NSCache<NSString, UIImage>()
    private let directory: URL
    /// Keys with a revalidation in flight, so a grid that draws the same
    /// cover in several places checks once.
    private var revalidating: Set<String> = []

    init() {
        memory.countLimit = 300
        memory.totalCostLimit = 96 * 1024 * 1024
        directory = OfflineStore.dataDirectory.appendingPathComponent("covers-v2", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // The v1 directory was keyed on `String.hashValue`, so nothing in it is
        // addressable from this process. Drop it rather than leak the space.
        try? FileManager.default.removeItem(
            at: OfflineStore.dataDirectory.appendingPathComponent("covers", isDirectory: true)
        )
    }

    private func diskURL(for key: String) -> URL {
        // Keys are URLs; hash so the filename is safe and bounded. It has to be
        // SHA256 rather than `String.hashValue` — Swift seeds that per process,
        // so hashed names never resolved after a relaunch and the disk tier
        // (which is the offline cover store) missed on every cold start.
        let digest = SHA256.hash(data: Data(key.utf8))
        return directory.appendingPathComponent(digest.map { String(format: "%02x", $0) }.joined())
    }

    /// Where the validator for `key`'s bytes lives — a sibling file, so a
    /// cache entry written before validators existed simply has none.
    private func etagURL(for key: String) -> URL {
        diskURL(for: key).appendingPathExtension("etag")
    }

    func image(for key: String) -> UIImage? {
        if let cached = memory.object(forKey: key as NSString) { return cached }
        let url = diskURL(for: key)
        guard let data = try? Data(contentsOf: url), let image = UIImage(data: data) else { return nil }
        memory.setObject(image, forKey: key as NSString, cost: data.count)
        return image
    }

    func store(_ image: UIImage, data: Data, for key: String, etag: String? = nil) {
        memory.setObject(image, forKey: key as NSString, cost: data.count)
        try? data.write(to: diskURL(for: key), options: .atomic)
        if let etag {
            try? Data(etag.utf8).write(to: etagURL(for: key), options: .atomic)
        } else {
            // A server that stopped publishing a validator must not leave the
            // previous one behind, or a later 304 would vouch for bytes it
            // never saw.
            try? FileManager.default.removeItem(at: etagURL(for: key))
        }
    }

    /// Ask the server whether a cached cover has changed, and replace it when
    /// it has. Called *after* the cached image is already on screen, so the
    /// render that triggers it pays nothing — the newer art lands on the next
    /// one.
    ///
    /// Skipped while offline, while the entry is younger than
    /// `revalidateAfter`, while it carries no validator to offer (a
    /// "revalidation" without one is a full refetch), and while another check
    /// of the same key is already running.
    func revalidate(_ key: String) async {
        // Reserved before the first `await`, not after. An actor suspends at
        // every suspension point and admits other callers, so checking the
        // set here and inserting past the connectivity hop let two callers
        // both pass the check and both fetch — the dedup this set exists for
        // never happened. `defer` releases on every exit, including the
        // early returns below.
        guard !revalidating.contains(key) else { return }
        revalidating.insert(key)
        defer { revalidating.remove(key) }

        guard await Connectivity.shared.isOnline else { return }
        let url = diskURL(for: key)
        guard let modified = try? url.resourceValues(forKeys: [.contentModificationDateKey])
            .contentModificationDate,
            Date().timeIntervalSince(modified) >= Self.revalidateAfter,
            let etag = try? String(contentsOf: etagURL(for: key), encoding: .utf8)
        else { return }
        guard let fresh = try? await APIClient.shared.conditionalData(for: key, ifNoneMatch: etag)
        else { return }

        guard let data = fresh.data, let decoded = UIImage(data: data) else {
            // A 304: the server confirming this copy is current, which is
            // exactly as good as having refetched it. The window has to be
            // restarted or the entry stays permanently overdue and every
            // later appearance of the same cover asks again — the
            // in-flight set only collapses *overlapping* checks, not
            // sequential ones.
            if fresh.isNotModified { markChecked(key) }
            return
        }
        store(decoded, data: data, for: key, etag: fresh.etag)
    }

    /// Restart an entry's freshness window without rewriting its bytes.
    private func markChecked(_ key: String) {
        try? FileManager.default.setAttributes(
            [.modificationDate: Date()], ofItemAtPath: diskURL(for: key).path
        )
    }

    /// Pull a cover into the cache without a view asking for it. Used when a
    /// book is downloaded, so its art is already on disk when the network goes.
    func prefetch(_ key: String) async {
        guard image(for: key) == nil else { return }
        guard let fetched = try? await APIClient.shared.conditionalData(for: key, ifNoneMatch: nil),
              let data = fetched.data,
              let decoded = UIImage(data: data)
        else { return }
        store(decoded, data: data, for: key, etag: fetched.etag)
    }

    /// Fetch a provider-hosted image (an absolute URL outside the Omnibus
    /// server). Same cache tiers, but the request must not carry the bearer
    /// token or wait on the Omnibus server's reachability — the host is a
    /// third party (Google Books, Open Library).
    func externalImage(for url: String) async -> UIImage? {
        if let cached = image(for: url) { return cached }
        // Provider metadata is untrusted input: only fetch http(s), and only
        // cache bodies a 2xx actually vouched for.
        guard let remote = URL(string: url),
              remote.scheme == "https" || remote.scheme == "http",
              let (data, response) = try? await URLSession.shared.data(from: remote),
              let http = response as? HTTPURLResponse,
              (200..<300).contains(http.statusCode),
              let decoded = UIImage(data: data)
        else { return nil }
        store(decoded, data: data, for: url)
        return decoded
    }

    func clearDisk() {
        try? FileManager.default.removeItem(at: directory)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        memory.removeAllObjects()
    }

    func diskBytes() -> Int64 {
        guard let contents = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: [.fileSizeKey]
        ) else { return 0 }
        return contents.reduce(0) { total, url in
            total + Int64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
    }
}

/// Loads a provider-hosted image from an absolute URL, showing `placeholder`
/// until it lands. A separate type from `RemoteImage` so no call site can
/// accidentally send the Omnibus bearer token to a third-party host.
struct ExternalImage<Placeholder: View>: View {
    let url: String?
    @ViewBuilder var placeholder: () -> Placeholder

    @State private var image: UIImage?

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFill()
                    .transition(.opacity)
            } else {
                placeholder()
            }
        }
        .task(id: url) {
            guard let url, !url.isEmpty else { return }
            guard let fetched = await ImageCache.shared.externalImage(for: url) else { return }
            guard url == self.url else { return }
            withAnimation(Motion.page) { image = fetched }
        }
    }
}

/// Loads an authenticated image, showing `placeholder` until it lands.
struct RemoteImage<Placeholder: View>: View {
    let path: String?
    /// Other paths for the *same* picture at a different size, tried from the
    /// cache when `path` isn't cached and can't be fetched.
    ///
    /// Sizes are separate files under separate keys, so a book browsed online
    /// has only the sizes that were actually drawn — the grid's, not the detail
    /// hero's. Offline that left the hero with no art at all, on a book whose
    /// cover was on screen a tap earlier. A smaller copy of the right picture
    /// beats the generated plate.
    var alternates: [String] = []
    @ViewBuilder var placeholder: () -> Placeholder

    @State private var image: UIImage?
    @State private var isLoading = false

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFill()
                    .transition(.opacity)
            } else {
                placeholder()
            }
        }
        .task(id: path) { await load() }
        // A cover the client skipped while the server was unreachable would
        // otherwise stay a blank plate until something rebuilt the view. Read
        // inline rather than stored — a stored property would land in the
        // memberwise init and make it private along with it.
        .onChange(of: Connectivity.shared.isOnline) { _, online in
            guard online, image == nil else { return }
            Task { await load() }
        }
    }

    private func load() async {
        guard let path, !path.isEmpty else {
            image = nil
            return
        }
        if let cached = await ImageCache.shared.image(for: path) {
            image = cached
            // Draw first, ask after: a cover replaced from another device
            // arrives on the next render rather than costing this one a
            // round-trip.
            Task.detached(priority: .utility) { await ImageCache.shared.revalidate(path) }
            return
        }
        // Draw another size of the same cover straight away if we have one, so
        // there is art on screen while the exact size is fetched — and art that
        // stays if the fetch can't happen at all.
        for alternate in alternates {
            guard let cached = await ImageCache.shared.image(for: alternate) else { continue }
            image = cached
            break
        }
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }

        guard let fetched = try? await APIClient.shared.conditionalData(for: path, ifNoneMatch: nil),
              let data = fetched.data,
              let decoded = UIImage(data: data)
        else { return }
        await ImageCache.shared.store(decoded, data: data, for: path, etag: fetched.etag)
        // Guard against a recycled cell resolving onto the wrong row.
        guard path == self.path else { return }
        withAnimation(Motion.page) { image = decoded }
    }
}
