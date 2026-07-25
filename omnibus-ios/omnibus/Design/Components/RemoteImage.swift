//  RemoteImage.swift
//  Authenticated image loading with a memory + disk cache.
//
//  `AsyncImage` can't attach the bearer header the cover endpoints require,
//  and covers are the single most-repeated request in the app, so they get a
//  real two-tier cache. The disk tier doubles as the offline cover store.

import SwiftUI
import UIKit

actor ImageCache {
    static let shared = ImageCache()

    private let memory = NSCache<NSString, UIImage>()
    private let directory: URL

    init() {
        memory.countLimit = 300
        memory.totalCostLimit = 96 * 1024 * 1024
        directory = OfflineStore.dataDirectory.appendingPathComponent("covers", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func diskURL(for key: String) -> URL {
        // Keys are URLs; hash so the filename is safe and bounded.
        directory.appendingPathComponent(String(format: "%02x", abs(key.hashValue)) + "-" + String(key.suffix(48).map { $0.isLetter || $0.isNumber ? $0 : "_" }))
    }

    func image(for key: String) -> UIImage? {
        if let cached = memory.object(forKey: key as NSString) { return cached }
        let url = diskURL(for: key)
        guard let data = try? Data(contentsOf: url), let image = UIImage(data: data) else { return nil }
        memory.setObject(image, forKey: key as NSString, cost: data.count)
        return image
    }

    func store(_ image: UIImage, data: Data, for key: String) {
        memory.setObject(image, forKey: key as NSString, cost: data.count)
        try? data.write(to: diskURL(for: key), options: .atomic)
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
    }

    private func load() async {
        guard let path, !path.isEmpty else {
            image = nil
            return
        }
        if let cached = await ImageCache.shared.image(for: path) {
            image = cached
            return
        }
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }

        guard let data = try? await APIClient.shared.data(for: path),
              let decoded = UIImage(data: data)
        else { return }
        await ImageCache.shared.store(decoded, data: data, for: path)
        // Guard against a recycled cell resolving onto the wrong row.
        guard path == self.path else { return }
        withAnimation(Motion.page) { image = decoded }
    }
}
