//  WidgetSnapshotWriter.swift
//  Builds the App Group snapshot the Home Screen widgets render from.
//
//  The extension gets a finished answer rather than a second reader on
//  `OfflineStore`: a timeline render has a tight budget, and a second SQLite
//  connection would be a concurrency problem the app does not have today.
//  Everything here reads the replica; the one thing that can touch the network
//  is pulling a cover that has never reached the device, and that is gated on
//  reachability so an offline pass cannot sit on a request timeout.

import UIKit
import WidgetKit

actor WidgetSnapshotWriter {
    static let shared = WidgetSnapshotWriter()

    /// Cap on the pre-rendered cover's long edge, in pixels. The largest a
    /// widget draws one is `systemSmall`'s, around 90pt — 300px covers it at
    /// 3x with room to spare, and keeps five books' art well under a megabyte
    /// in a container both processes have to page in.
    private static let thumbMaxPixels: CGFloat = 300

    /// How long a pass waits on the Continue rail before composing from the
    /// replica instead. Longer than the reader's `PositionSync.openDeadline`
    /// — nobody is watching a snapshot pass the way they are watching a book
    /// open — but short enough to leave the background assertion its time.
    private static let railDeadline: Duration = .seconds(3)

    /// The pass currently running or queued behind one.
    private var pass: Task<Void, Never>?

    /// Rebuild the snapshot and tell WidgetKit to redraw.
    ///
    /// Passes are **serialized**, and that is load-bearing rather than tidy.
    /// A build takes several `await`s (the replica, the mirror, possibly a
    /// cover fetch), and sign-out runs its own refresh after two network calls
    /// of its own — so an unordered pair let a build that started while signed
    /// in finish *after* the signed-out one, re-publishing the previous
    /// account's titles and cover art to a surface outside the app's sandbox
    /// and re-writing the thumbs the sign-out had just pruned. Chaining makes
    /// the last caller the last writer.
    func refresh() async {
        let previous = pass
        let mine = Task { [previous] in
            _ = await previous?.value
            await Self.runPass()
        }
        pass = mine
        await mine.value
    }

    private static func runPass() async {
        var snapshot = await build()

        // Re-checked after the build, not only before it. The build is long
        // enough for a sign-out to land inside it, and publishing books for an
        // account that is no longer signed in is the one outcome this file
        // exists to prevent.
        if snapshot.state == .ready, await !APIClient.shared.hasToken() {
            snapshot = .empty(.signedOut)
        }

        WidgetStore.save(snapshot)
        WidgetStore.pruneThumbs(keeping: Set(snapshot.books.compactMap(\.thumb)))
        WidgetCenter.shared.reloadTimelines(ofKind: WidgetKind.continueReading)
    }

    private static func build() async -> WidgetSnapshot {
        let hasToken = await APIClient.shared.hasToken()
        guard hasToken else { return .empty(.signedOut) }

        await refreshRail(hasToken: hasToken)
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

    /// Whether a pass should spend a rail pull: a session to read with, and a
    /// network to read over. `internal` so the rule is testable.
    ///
    /// The token is a parameter rather than an ordering assumption. `build`
    /// guards on it above, so passing it here reads redundant — but that guard
    /// is the only thing standing between a sign-out pass and an authenticated
    /// read, and an ordering property holds only until someone moves a line.
    /// As a term of the rule it cannot be lost that way.
    static func shouldPullRail(hasToken: Bool, isOnline: Bool) -> Bool {
        hasToken && isOnline
    }

    /// Pull the Continue rail through its live read, for the write into the
    /// cache the read performs — the values are `build`'s business.
    ///
    /// A position this device just wrote is deliberately incomplete: the
    /// reader saves a CFI and the server derives the whole-book percent off
    /// the request path, so the optimistic row carries no percent and a card
    /// built from it draws no bar. Half the passes used to pull first and half
    /// didn't, and the ones that didn't are the two that run right after a
    /// write — a close, a backgrounding — so the Home Screen kept the barless
    /// snapshot until the app was next opened. Owning the pull here is what
    /// stops the next call site from reintroducing that.
    ///
    /// Gated on reachability like the cover fetch below: offline this stays a
    /// pure replica read rather than paying a request timeout inside a
    /// background assertion that cancels the whole pass when it runs out.
    ///
    /// Bounded for the same reason, and the bound **cancels** rather than
    /// abandons. Waiting unbounded would put a hung request in front of the
    /// publish on the way to the background, where a pass the expiration
    /// handler cancels writes no snapshot at all and the Home Screen keeps an
    /// older one. Letting the read run on past the bound — `firstResult`'s
    /// shape — would break this actor's one guarantee, that the last caller is
    /// the last writer: a pull outliving its pass still carries the bearer it
    /// went out with, so it can land the previous account's rail in the replica
    /// after a sign-out. Cancelling costs only a request that hadn't answered,
    /// since `Cache.live` caches whatever it has already fetched regardless.
    private static func refreshRail(hasToken: Bool) async {
        guard await shouldPullRail(hasToken: hasToken, isOnline: Connectivity.shared.isOnline)
        else { return }
        let pull = Task {
            for await _ in UserDataService.recentProgress().values() {}
        }
        // `try`, not `try?`: cancelling the deadline has to *skip* the cancel,
        // and a swallowed `CancellationError` would fall through to it instead.
        // Harmless in this order — the pull has already finished by then — but
        // it reads as a guard it isn't, and one swapped line would make it a
        // real spurious cancel.
        let deadline = Task {
            try await Task.sleep(for: railDeadline)
            pull.cancel()
        }
        await pull.value
        deadline.cancel()
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
            fraction: fraction(for: point, format: format),
            secondsRemaining: secondsRemaining(for: point, format: format),
            updatedAt: Date(timeIntervalSince1970: TimeInterval(point.record.updatedAt)),
            fileID: format == .audio ? point.record.bookFileID : nil,
            thumb: await thumb(for: book)
        )
    }

    /// The record's own honest fraction, but never a listening fraction on a
    /// card that is no longer presenting itself as one — the same guard the
    /// Continue hero applies. `internal` so the rule is testable.
    static func fraction(for point: ResumePoint, format: ProgressFormat) -> Double? {
        format == point.record.format ? point.fraction : nil
    }

    /// The wall-clock wait at the reader's saved speed, matching the player's
    /// own "left" readout. Clamped first: a position past the reported total
    /// (stale metadata) must read as none left, not as a negative span.
    /// `internal` so the arithmetic is testable.
    static func secondsRemaining(for point: ResumePoint, format: ProgressFormat) -> Double? {
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
        if image == nil, await Connectivity.shared.isOnline {
            // Guarded on reachability because `ImageCache.prefetch` has no
            // guard of its own, and `APIClient`'s fail-fast window only opens
            // once something has *already* timed out — so an offline pass
            // would pay a full request timeout per book, serially, inside a
            // background assertion that cancels the whole pass when it runs
            // out. Offline this stays a pure replica read.
            await ImageCache.shared.prefetch(key)
            image = await ImageCache.shared.image(for: key)
        }

        // Whatever the group already holds beats nothing. A cover that cannot
        // be re-resolved this pass — offline, or after the image cache was
        // cleared — must not be pruned out from under a widget that is
        // currently drawing it, or an Airplane Mode render silently degrades
        // every book to its plate and stays there until the next online pass.
        guard let image else { return WidgetStore.existingThumbName(for: book.uuid) }

        let ext = hasAlpha(image) ? "png" : "jpg"
        let name = WidgetStore.thumbName(for: book.uuid, ext: ext)
        guard await needsRewrite(name: name, source: key) else { return name }

        guard let data = encode(image, ext: ext) else {
            return WidgetStore.existingThumbName(for: book.uuid)
        }
        WidgetStore.writeThumb(data, named: name)
        return name
    }

    /// Whether the group's copy is older than the cached cover it came from.
    ///
    /// `refresh()` is wired to eight call sites, so a session of foreground →
    /// open → close → background is four passes over the same five books.
    /// Without this each one re-decoded, re-scaled and re-encoded every cover
    /// and rewrote it, including inside the suspension race on the way out.
    private static func needsRewrite(name: String, source key: String) async -> Bool {
        guard let written = WidgetStore.thumbModified(named: name) else { return true }
        guard let cached = await ImageCache.shared.diskModified(for: key) else { return false }
        return written < cached
    }

    /// Downscale to the widget's needs and encode.
    ///
    /// PNG when the cover carries alpha — some are transparent, and flattening
    /// one onto JPEG's implicit black is how a cover that reads fine in the app
    /// becomes a dark slab on the Home Screen. The file is named for whichever
    /// this produced, so nothing downstream has to infer the type from bytes.
    private static func encode(_ image: UIImage, ext: String) -> Data? {
        let longest = max(image.size.width, image.size.height)
        guard longest > 0 else { return nil }

        let scaled = longest <= thumbMaxPixels ? image : resize(image, by: thumbMaxPixels / longest)
        return ext == "png" ? scaled.pngData() : scaled.jpegData(compressionQuality: 0.85)
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
