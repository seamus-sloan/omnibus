//  RemoteImage.swift
//  Authenticated image loading with a memory + disk cache.
//
//  `AsyncImage` can't attach the bearer header the cover endpoints require,
//  and covers are the single most-repeated request in the app, so they get a
//  real two-tier cache. The disk tier doubles as the offline cover store.

import CryptoKit
import SwiftUI
import UIKit

/// What a cached entry's check-back with the server should be.
///
/// Split out of `ImageCache.revalidate` so the policy can be covered without a
/// filesystem or a server standing behind it — the actor around it is neither.
enum RevalidationPlan: Equatable {
    /// Inside the freshness window. Asking again would cost one request per
    /// visible cover, every time a grid scrolled it back into view.
    case skip
    /// Ask cheaply: the entry has a validator, so the server can answer 304
    /// and send no body at all.
    case conditional(etag: String)
    /// Ask outright. The entry has no validator to offer, so this costs a
    /// full body — once, because the answer carries the validator that puts
    /// the entry back on the cheap path.
    case unconditional
}

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
    /// How many times each key has been invalidated.
    ///
    /// A fetch carries the count it started under, and [`store`] refuses a
    /// write whose count has moved — which is the only thing separating "these
    /// bytes are current" from "these bytes were current when I asked". A cover
    /// write invalidates the key while a fetch for the *previous* cover is
    /// still in flight; without this the reply lands afterwards and re-creates
    /// the entry the write just deleted, with the superseded art and a fresh
    /// mtime that also restarts the revalidation window (#2547).
    private var generations: [String: Int] = [:]
    /// Bumped by [`clearDisk`], which invalidates every key at once including
    /// the ones no per-key count has been kept for. Added to the per-key count
    /// rather than replacing it, so a key's generation only ever rises.
    private var globalGeneration = 0

    /// `directory` is for tests, which need a cache of their own: several of
    /// them invalidate and clear, and doing that to the one shared instance
    /// would delete entries a sibling running in parallel is asserting on.
    init(directory: URL? = nil) {
        memory.countLimit = 300
        memory.totalCostLimit = 96 * 1024 * 1024
        let resolved =
            directory
            ?? OfflineStore.dataDirectory.appendingPathComponent("covers-v2", isDirectory: true)
        self.directory = resolved
        try? FileManager.default.createDirectory(at: resolved, withIntermediateDirectories: true)
        guard directory == nil else { return }
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

    /// When this key's bytes were last written to disk.
    ///
    /// For callers holding a *derived* copy somewhere else — the widget
    /// snapshot's pre-rendered covers live in the App Group, and only need
    /// re-encoding when the cover behind them has actually moved.
    func diskModified(for key: String) -> Date? {
        try? diskURL(for: key)
            .resourceValues(forKeys: [.contentModificationDateKey])
            .contentModificationDate
    }

    /// What `key`'s bytes would have to have been fetched under to still be
    /// current. Capture this *before* a fetch and hand it back to [`store`].
    func generation(for key: String) -> Int {
        globalGeneration + generations[key, default: 0]
    }

    /// Commit fetched bytes, unless the key was invalidated while they were in
    /// flight.
    ///
    /// `generation` is what the caller captured before it asked. A write whose
    /// generation has since moved describes a cover that has already been
    /// replaced, and committing it would undo the invalidation the replacement
    /// performed. Passing `nil` skips the check, and is only right for bytes
    /// that cannot have raced one — a caller that did not fetch them.
    @discardableResult
    func store(
        _ image: UIImage,
        data: Data,
        for key: String,
        etag: String? = nil,
        generation: Int? = nil
    ) -> Bool {
        if let generation, generation != self.generation(for: key) { return false }
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
        return true
    }

    /// Ask the server whether a cached cover has changed, and replace it when
    /// it has. Called *after* the cached image is already on screen, so the
    /// render that triggers it pays nothing.
    ///
    /// Returns the replacement when there is one, so the caller can swap it in
    /// without waiting to be rebuilt. Returning nothing and leaving the caller
    /// to find out later is what made a stale entry look permanent: the bytes
    /// were corrected on disk while the view kept its decoded copy, so the fix
    /// only became visible on the *next* view creation — indistinguishable
    /// from never healing, to anyone checking once per launch (#2547).
    ///
    /// Skipped while offline, while another check of the same key is already
    /// running, and while [`plan`] says the entry is still fresh.
    @discardableResult
    func revalidate(_ key: String) async -> UIImage? {
        // Reserved before the first `await`, not after. An actor suspends at
        // every suspension point and admits other callers, so checking the
        // set here and inserting past the connectivity hop let two callers
        // both pass the check and both fetch — the dedup this set exists for
        // never happened. `defer` releases on every exit, including the
        // early returns below.
        guard !revalidating.contains(key) else { return nil }
        revalidating.insert(key)
        defer { revalidating.remove(key) }

        guard await Connectivity.shared.isOnline else { return nil }
        let url = diskURL(for: key)
        guard let modified = try? url.resourceValues(forKeys: [.contentModificationDateKey])
            .contentModificationDate
        else { return nil }
        let stored = try? String(contentsOf: etagURL(for: key), encoding: .utf8)
        let ifNoneMatch: String?
        switch Self.plan(age: Date().timeIntervalSince(modified), etag: stored) {
        case .skip: return nil
        case .conditional(let etag): ifNoneMatch = etag
        case .unconditional: ifNoneMatch = nil
        }
        // Captured before the request, like every other fetch here: a cover
        // write during it invalidates the key, and both answers below describe
        // the copy that write replaced.
        let generation = generation(for: key)
        guard let fresh = try? await APIClient.shared.conditionalData(
            for: key, ifNoneMatch: ifNoneMatch
        ) else { return nil }

        guard let data = fresh.data, let decoded = UIImage(data: data) else {
            // A 304: the server confirming this copy is current, which is
            // exactly as good as having refetched it. The window has to be
            // restarted or the entry stays permanently overdue and every
            // later appearance of the same cover asks again — the
            // in-flight set only collapses *overlapping* checks, not
            // sequential ones. Unless the copy it vouched for is already gone,
            // in which case there is nothing left to call fresh.
            if fresh.isNotModified, generation == self.generation(for: key) { markChecked(key) }
            return nil
        }
        guard store(decoded, data: data, for: key, etag: fresh.etag, generation: generation) else {
            return nil
        }
        return decoded
    }

    /// Whether an entry `age` seconds old, holding `etag` (or nothing), is due
    /// a check-back — and whether that check can be a conditional one.
    ///
    /// A validator-less entry is checked anyway. Skipping those made "we
    /// cannot ask cheaply" mean "we never ask", and nothing ever restored the
    /// validator — so the entry stayed on those bytes for the life of the
    /// install, however many times the cover behind it changed. That is how a
    /// grid tile and the detail hero came to disagree about one book (#2539):
    /// the hero's full-cover response always carried an ETag and healed, while
    /// a tile fetched before its thumbnail existed was served the stand-in
    /// cover, which carried none.
    static func plan(age: TimeInterval, etag: String?) -> RevalidationPlan {
        guard age >= revalidateAfter else { return .skip }
        guard let etag, !etag.isEmpty else { return .unconditional }
        return .conditional(etag: etag)
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
        let generation = generation(for: key)
        guard let fetched = try? await APIClient.shared.conditionalData(for: key, ifNoneMatch: nil),
              let data = fetched.data,
              let decoded = UIImage(data: data)
        else { return }
        store(decoded, data: data, for: key, etag: fetched.etag, generation: generation)
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

    /// Drop one key's cached bytes and its validator.
    ///
    /// Needed because `revalidate` only checks back every `revalidateAfter`
    /// seconds: after *this* device replaces its own avatar, the new picture
    /// would otherwise not appear for up to five minutes. Other devices still
    /// wait for the normal window, same as covers.
    func invalidate(_ key: String) {
        // Before the removals, so a fetch that resolves between them and the
        // next `store` is already refused.
        generations[key, default: 0] += 1
        memory.removeObject(forKey: key as NSString)
        try? FileManager.default.removeItem(at: diskURL(for: key))
        try? FileManager.default.removeItem(at: etagURL(for: key))
    }

    func clearDisk() {
        globalGeneration += 1
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
            // Draw first, ask after: the cached art is already on screen, so
            // the check costs this render nothing. Awaited rather than
            // detached so a replacement swaps in here instead of waiting for
            // something to rebuild the view.
            if let refreshed = await ImageCache.shared.revalidate(path), path == self.path {
                withAnimation(Motion.page) { image = refreshed }
            }
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

        // Captured before the request: a cover write landing during it
        // invalidates this key, and these bytes are then the cover it
        // replaced. Committing them would undo that invalidation.
        let generation = await ImageCache.shared.generation(for: path)
        guard let fetched = try? await APIClient.shared.conditionalData(for: path, ifNoneMatch: nil),
              let data = fetched.data,
              let decoded = UIImage(data: data)
        else { return }
        guard await ImageCache.shared.store(
            decoded, data: data, for: path, etag: fetched.etag, generation: generation
        ) else { return }
        // Guard against a recycled cell resolving onto the wrong row.
        guard path == self.path else { return }
        withAnimation(Motion.page) { image = decoded }
    }
}
