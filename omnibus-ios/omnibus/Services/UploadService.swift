//  UploadService.swift
//  The two-step "add your own books" ingest: inspect a picked file for its
//  embedded metadata, then commit it with the metadata the user confirmed.
//
//  Both steps are online-only and throw on failure. An upload is library-wide
//  state — it files bytes into a shared library and kicks a reindex — so it
//  never queues in the outbox (rule 08, test 1), and the sheet disables its
//  controls while offline rather than letting the request fail after the fact.

import Foundation

/// What the user confirmed on the confirm sheet, ready to commit.
struct UploadConfirmation: Equatable, Sendable {
    var title: String
    var author: String
    var series: String = ""
    var seriesIndex: String = ""
}

enum UploadService {
    /// Parent of every per-batch staging directory, so a sweep can find
    /// leftovers from a crash or a dismissal that outran its cleanup.
    private static var stagingRoot: URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("omnibus-uploads", isDirectory: true)
    }

    /// Allocate (but do not create) a staging directory for one batch.
    ///
    /// Handed out before any copying so the caller can register it as live
    /// first: a `stage` that throws part-way has already written the parts it
    /// got through, and a directory nobody holds is a directory nobody frees.
    static func makeStagingDirectory() -> URL {
        stagingRoot.appendingPathComponent(UUID().uuidString, isDirectory: true)
    }

    /// Copy a batch's picked file(s) into `directory`, returning the batch
    /// rebased onto the copies.
    ///
    /// Two reasons, both about the confirm step: the picker's URLs are
    /// security-scoped and that access must be released promptly, while the
    /// human pause before the reader hits Add is unbounded.
    ///
    /// The copy runs detached because this target builds with approachable
    /// concurrency, under which a `nonisolated async` function runs on its
    /// *caller's* actor. A multi-hundred-megabyte audiobook copied on the main
    /// thread freezes the UI for the length of the copy.
    static func stage(_ batch: UploadBatch, into directory: URL) async throws -> UploadBatch {
        try await Task.detached(priority: .userInitiated) {
            try stageOnDisk(batch, into: directory)
        }.value
    }

    private static func stageOnDisk(
        _ batch: UploadBatch, into directory: URL
    ) throws -> UploadBatch {
        var staged: [URL] = []
        for (index, source) in batch.urls.enumerated() {
            try Task.checkCancellation()
            // Each part gets its own numbered slot so a copy can never collide,
            // which lets the *original* filename go out on the wire. Renaming
            // client-side to dodge a collision was worse than useless: the
            // server already dedupes, and a `-N` suffix sorts before the bare
            // stem ('-' < '.'), which is the tiebreak it orders parts by.
            let slot = directory.appendingPathComponent("\(index)", isDirectory: true)
            try FileManager.default.createDirectory(at: slot, withIntermediateDirectories: true)

            let scoped = source.startAccessingSecurityScopedResource()
            defer { if scoped { source.stopAccessingSecurityScopedResource() } }
            let destination = slot.appendingPathComponent(source.lastPathComponent)
            try FileManager.default.copyItem(at: source, to: destination)
            staged.append(destination)
        }
        return UploadBatch(kind: batch.kind, urls: staged)
    }

    /// Delete one staged copy once its upload has finished, succeeded or not.
    static func discard(_ directory: URL) {
        Task.detached(priority: .utility) { discardNow(directory) }
    }

    /// The synchronous body of [`discard`], so a caller that must observe the
    /// deletion (a test) does not have to race the detached task.
    static func discardNow(_ directory: URL) {
        try? FileManager.default.removeItem(at: directory)
    }

    /// Delete staged copies except those in `keeping`.
    ///
    /// Staging outlives its owner when cleanup cannot run — a crash, a kill
    /// mid-confirm — and iOS does not purge `tmp` while the app runs. But a
    /// blind sweep of the root is worse than the leak: an upload that outlived
    /// its sheet is still reading from one of these directories.
    /// `root` is a seam for tests: the sweep is destructive over a whole
    /// directory, so a test that swept the real root would delete staging that
    /// a concurrently-running test still owns.
    static func sweepStaging(keeping live: Set<URL>, in root: URL? = nil) {
        let root = root ?? stagingRoot
        let fm = FileManager.default
        // Compared by directory name, not by URL: `temporaryDirectory` and the
        // paths `contentsOfDirectory` returns differ by the /var -> /private/var
        // symlink, and neither `standardizedFileURL` nor string equality sees
        // through that — which would have swept every live directory.
        let keep = Set(live.map(\.lastPathComponent))
        guard
            let entries = try? fm.contentsOfDirectory(
                at: root, includingPropertiesForKeys: nil
            )
        else { return }
        for entry in entries where !keep.contains(entry.lastPathComponent) {
            try? fm.removeItem(at: entry)
        }
    }

    /// Parse the picked file(s) and return the metadata to pre-fill the confirm
    /// form with. Nothing is written — the server discards its tempfile.
    static func inspect(_ batch: UploadBatch) async throws -> UploadConfirmation {
        let (body, boundary) = try await encodedBody(for: batch, fields: [])
        switch batch.kind {
        case .ebook:
            let inspection: UploadInspection = try await APIClient.shared.upload(
                batch.kind.inspectPath, body: body, boundary: boundary
            )
            return UploadConfirmation(
                title: inspection.title ?? "",
                author: inspection.author ?? "",
                series: inspection.series ?? "",
                seriesIndex: inspection.seriesIndex ?? ""
            )
        case .audiobook:
            let inspection: AudiobookInspection = try await APIClient.shared.upload(
                batch.kind.inspectPath, body: body, boundary: boundary
            )
            return UploadConfirmation(
                title: inspection.title ?? "", author: inspection.author ?? ""
            )
        }
    }

    /// File the book into the library under the confirmed metadata and return
    /// its durable uuid. The commit handlers reject a body without `title` and
    /// `author`, so [`UploadFlow.commitFields`] always sends both.
    static func commit(
        _ batch: UploadBatch, as confirmation: UploadConfirmation
    ) async throws -> UploadCommitResult {
        let fields = UploadFlow.commitFields(
            kind: batch.kind,
            title: confirmation.title,
            author: confirmation.author,
            series: confirmation.series,
            seriesIndex: confirmation.seriesIndex
        )
        let (body, boundary) = try await encodedBody(for: batch, fields: fields)
        // Invalidation is the caller's: `UploadManager` decides from the
        // outcome whether the library can have changed, and coalesces one
        // resync per pick rather than one per book.
        return try await APIClient.shared.upload(
            batch.kind.commitPath, body: body, boundary: boundary
        )
    }

    /// Drop every cached read a new book invalidates, and resync the offline
    /// mirror. Named rather than inlined so the next write that adds a book has
    /// one place to call. `CheckInView` clears an overlapping but narrower
    /// set for its own book-creating writes; folding it in here is worth
    /// doing and is tracked separately.
    static func invalidateLibrary() async {
        let store = OfflineStore.shared
        // The paged listing is the real library cache; `CacheKey.library`
        // itself is never written by anything, so deleting it is a no-op.
        await store.cacheDeletePrefix(CacheKey.libraryPagePrefix)
        await store.cacheDelete(CacheKey.authors)
        await store.cacheDelete(CacheKey.series)
        // A new book's subjects feed both of these.
        await store.cacheDelete(CacheKey.tags)
        await store.cacheDelete(CacheKey.genres)
        // Shelf counts and preview covers move, and so does the membership the
        // shelf page itself lists — a rule-matching upload joins smart shelves.
        await store.cacheDelete(CacheKey.shelves)
        await store.cacheDelete(CacheKey.shelfPreviews)
        await store.cacheDeletePrefix("shelf:")
        await store.cacheDeletePrefix("shelf_page:")
        // The SQLite mirror is not a cache key — it backs offline paging and
        // search, and its own sync self-throttles to once per five minutes.
        await LibraryIndex.shared.sync(force: true)
    }

    /// Read the staged parts and encode the multipart body, all off the caller's
    /// actor.
    ///
    /// Encoding here rather than inside `APIClient` matters: `upload` is an
    /// actor method, so concatenating a multi-hundred-megabyte body there would
    /// hold the actor every other request in the app funnels through.
    private static func encodedBody(
        for batch: UploadBatch, fields: [(name: String, value: String)]
    ) async throws -> (body: Data, boundary: String) {
        try await Task.detached(priority: .userInitiated) {
            let files = try batch.urls.map(loadPart)
            let boundary = MultipartBody.makeBoundary()
            let body = MultipartBody.encode(boundary: boundary, fields: fields, files: files)
            return (body, boundary)
        }.value
    }

    /// Read one staged part off disk. By the time either step runs the files
    /// live in this app's temporary directory — [`stage`] copied them out of
    /// the picker's security-scoped location — so no scope is claimed here and
    /// a slow confirm step cannot outlive one.
    ///
    /// `.mappedIfSafe` keeps the source pages file-backed and evictable instead
    /// of dirty anonymous memory; the file is one this app just wrote to its own
    /// tmp directory and nothing mutates it, which is what makes mapping safe.
    private static func loadPart(_ url: URL) throws -> MultipartFile {
        let name = url.lastPathComponent
        return MultipartFile(
            fileName: name,
            mimeType: UploadFlow.mimeType(for: name),
            data: try Data(contentsOf: url, options: .mappedIfSafe)
        )
    }
}
