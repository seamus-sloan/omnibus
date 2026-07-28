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

    private let memory = NSCache<NSString, UIImage>()
    private let directory: URL
    /// Keys already revalidated against the server this launch — see
    /// `refreshIfSuperseded`.
    private var revalidated: Set<String> = []

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
        let sidecar = etagURL(for: key)
        if let etag {
            try? Data(etag.utf8).write(to: sidecar, options: .atomic)
        } else {
            // A server too old to send one must not leave a stale tag behind
            // that a later revalidation would quote back at it.
            try? FileManager.default.removeItem(at: sidecar)
        }
    }

    /// The validator the copy on disk was stored under.
    private func etagURL(for key: String) -> URL {
        diskURL(for: key).appendingPathExtension("etag")
    }

    private func etag(for key: String) -> String? {
        guard let data = try? Data(contentsOf: etagURL(for: key)),
              let etag = String(data: data, encoding: .utf8),
              !etag.isEmpty
        else { return nil }
        return etag
    }

    /// Ask the server whether the cached copy of `key` is still the one it
    /// holds, at most once per key per launch.
    ///
    /// Returns the replacement when it is not, and `nil` when it is current,
    /// when the check was already made, or when the server could not be
    /// reached — all of which mean the copy on disk stands. The once-per-launch
    /// bound is what keeps this affordable: a shelf draws the same cover on
    /// every scroll pass, and a request behind each of those would put dozens
    /// of round trips behind a flick. A cover changed on another device still
    /// arrives — on the next launch, or the first time that image is drawn.
    ///
    /// An entry cached before validators were recorded has nothing to ask
    /// with, so it refetches once and is conditional from then on.
    func refreshIfSuperseded(_ key: String) async -> UIImage? {
        guard !revalidated.contains(key), await Connectivity.shared.isOnline else { return nil }
        let outcome: (data: Data, etag: String?)?
        do {
            outcome = try await APIClient.shared.conditionalData(for: key, ifNoneMatch: etag(for: key))
        } catch {
            // Only a definitive answer counts as checked; an unreachable
            // server should be asked again once the connection is back.
            return nil
        }
        revalidated.insert(key)
        guard let outcome, let decoded = UIImage(data: outcome.data) else { return nil }
        store(decoded, data: outcome.data, for: key, etag: outcome.etag)
        return decoded
    }

    /// Pull a cover into the cache without a view asking for it. Used when a
    /// book is downloaded, so its art is already on disk when the network goes.
    func prefetch(_ key: String) async {
        guard image(for: key) == nil else { return }
        guard let outcome = try? await APIClient.shared.conditionalData(for: key, ifNoneMatch: nil),
              let decoded = UIImage(data: outcome.data)
        else { return }
        revalidated.insert(key)
        store(decoded, data: outcome.data, for: key, etag: outcome.etag)
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
            // Drawn first, checked second: the reader sees their cover
            // immediately, and a cover replaced from another device swaps in
            // behind it rather than holding the plate blank on a round trip.
            if let refreshed = await ImageCache.shared.refreshIfSuperseded(path) {
                guard path == self.path else { return }
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

        guard let outcome = try? await APIClient.shared.conditionalData(for: path, ifNoneMatch: nil),
              let decoded = UIImage(data: outcome.data)
        else { return }
        await ImageCache.shared.store(decoded, data: outcome.data, for: path, etag: outcome.etag)
        // Guard against a recycled cell resolving onto the wrong row.
        guard path == self.path else { return }
        withAnimation(Motion.page) { image = decoded }
    }
}
