//  WidgetSnapshotWriter.swift
//  Builds the App Group snapshot the Home Screen widgets render from.
//
//  The extension gets a finished answer rather than a second reader on
//  `OfflineStore`: a timeline render has a tight budget, and a second SQLite
//  connection would be a concurrency problem the app does not have today.
//  Everything here reads the replica only — a snapshot write must never block
//  on the network, since most of them happen on the way to being suspended.

import UIKit
import WidgetKit

enum WidgetSnapshotWriter {
    /// Cap on the pre-rendered cover's long edge, in pixels. The largest a
    /// widget draws one is `systemSmall`'s, around 90pt — 300px covers it at
    /// 3x with room to spare, and keeps five books' art well under a megabyte
    /// in a container both processes have to page in.
    private static let thumbMaxPixels: CGFloat = 300

    /// Rebuild the snapshot and tell WidgetKit to redraw.
    ///
    /// Safe to call from anywhere and at any phase — it resolves the empty
    /// states itself rather than expecting callers to know which one applies.
    static func refresh() async {
        let snapshot = await build()
        WidgetStore.save(snapshot)
        WidgetStore.pruneThumbs(keeping: Set(snapshot.books.compactMap(\.thumb)))
        WidgetCenter.shared.reloadTimelines(ofKind: WidgetKind.continueReading)
    }

    private static func build() async -> WidgetSnapshot {
        guard await APIClient.shared.hasToken() else { return .empty(.signedOut) }

        let points: [ResumePoint] = await Cache.cachedOnly(CacheKey.recentProgress) ?? []
        guard !points.isEmpty else {
            // Two different things to say, and the mirror is what tells them
            // apart: a library with no books wants "add some", one with books
            // but nothing open wants "start reading".
            let populated = await LibraryIndex.shared.isPopulated()
            return .empty(populated ? .nothingInProgress : .emptyLibrary)
        }

        var books: [WidgetBook] = []
        // Serial rather than a task group: the thumbs come out of the image
        // cache and, when one is missing, one request each. Five of those in
        // parallel on the way to suspension is a burst that buys nothing —
        // the widget redraws when the write lands either way.
        for point in points.prefix(WidgetSnapshot.maxBooks) {
            books.append(await entry(for: point))
        }
        return WidgetSnapshot(state: .ready, books: books, generatedAt: Date())
    }

    private static func entry(for point: ResumePoint) async -> WidgetBook {
        // The mirror carries the whole library and is refreshed as a unit, so
        // it holds the newer title after a metadata edit; the copy embedded in
        // the resume payload is the fallback for a book the mirror has not
        // seen (a first launch, an interrupted pass).
        let book = await LibraryIndex.shared.book(uuid: point.record.bookUUID) ?? point.book
        let format = book.resumeFormat(for: point.record.format)
        let tone = CoverIdentity(book).tone

        return WidgetBook(
            bookUUID: book.uuid,
            format: WidgetFormat(format),
            title: book.displayTitle,
            author: book.authorDisplay,
            tone: WidgetBook.Tone(l: tone.l, c: tone.c, h: tone.h),
            // Not a listening fraction on a card that is no longer presenting
            // itself as one — the same guard the Continue hero applies.
            fraction: format == point.record.format ? point.fraction : nil,
            secondsRemaining: secondsRemaining(for: point, format: format),
            updatedAt: Date(timeIntervalSince1970: TimeInterval(point.record.updatedAt)),
            fileID: format == .audio ? point.record.bookFileID : nil,
            thumb: await thumb(for: book)
        )
    }

    /// The wall-clock wait at the reader's saved speed, matching the player's
    /// own "left" readout. Clamped first: a position past the reported total
    /// (stale metadata) must read as none left, not as a negative span.
    private static func secondsRemaining(
        for point: ResumePoint, format: ProgressFormat
    ) -> Double? {
        guard format == .audio, point.isAudio,
              let total = point.totalDurationSeconds,
              let position = point.record.audioPositionSeconds
        else { return nil }
        return Format.atRate(max(0, total - position), rate: point.playbackRate ?? 1.0)
    }

    // MARK: - Cover art

    /// Put this book's cover in the App Group, so the extension never fetches
    /// an image during a timeline render, and answer with the name it was
    /// filed under.
    private static func thumb(for book: Book) async -> String? {
        guard book.coverURL != nil else { return nil }
        let key = "/api/thumbs/\(book.uuid)/\(ThumbSize.md.rawValue)"

        var image = await ImageCache.shared.image(for: key)
        if image == nil {
            // No-ops offline and when the cache already holds it, so this is
            // only ever the first snapshot after a book reaches the rail.
            await ImageCache.shared.prefetch(key)
            image = await ImageCache.shared.image(for: key)
        }
        guard let image, let data = encode(image) else { return nil }

        let name = WidgetStore.thumbName(for: book.uuid)
        WidgetStore.writeThumb(data, named: name)
        return name
    }

    /// Downscale to the widget's needs and encode.
    ///
    /// JPEG unless the cover carries alpha — some are transparent PNGs, and
    /// flattening one onto JPEG's implicit black is how a cover that reads
    /// fine in the app becomes a dark slab on the Home Screen.
    private static func encode(_ image: UIImage) -> Data? {
        let longest = max(image.size.width, image.size.height)
        guard longest > 0 else { return nil }

        let scaled = longest <= thumbMaxPixels ? image : resize(image, by: thumbMaxPixels / longest)
        return hasAlpha(scaled) ? scaled.pngData() : scaled.jpegData(compressionQuality: 0.85)
    }

    private static func resize(_ image: UIImage, by scale: CGFloat) -> UIImage {
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        let format = UIGraphicsImageRendererFormat.default()
        // The source was decoded from data, so its `size` is already in
        // pixels; leaving the renderer on the screen's scale would render at
        // 2–3x the size asked for.
        format.scale = 1
        format.opaque = !hasAlpha(image)
        return UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
    }

    private static func hasAlpha(_ image: UIImage) -> Bool {
        switch image.cgImage?.alphaInfo {
        case .first, .last, .premultipliedFirst, .premultipliedLast: true
        default: false
        }
    }
}
