//  UploadService.swift
//  The two-step "add your own books" ingest: inspect a picked file for its
//  embedded metadata, then commit it with the metadata the user confirmed.
//
//  Both steps are online-only and throw on failure. An upload is a command —
//  it files bytes into a shared library and kicks a reindex — so it never
//  queues in the outbox (rule 08, tests 1 and 2), and the sheet disables the
//  control while offline rather than letting the request fail after the fact.

import Foundation

/// What the user confirmed on the confirm sheet, ready to commit.
struct UploadConfirmation: Equatable, Sendable {
    var title: String
    var author: String
    var series: String = ""
    var seriesIndex: String = ""
}

enum UploadService {
    /// Copy a batch's picked file(s) into this app's temporary directory,
    /// returning the batch rebased onto the copies plus the directory holding
    /// them.
    ///
    /// Two reasons, both about the confirm step: the picker's URLs are
    /// security-scoped and that access must be released promptly, while the
    /// human pause before the user hits Add is unbounded. Copying also keeps a
    /// large book on disk across that pause instead of resident in a `Data`.
    ///
    /// The copy runs detached because this target builds with approachable
    /// concurrency, under which a `nonisolated async` function runs on its
    /// *caller's* actor — here, the view. A multi-hundred-megabyte audiobook
    /// copied on the main thread freezes the UI for the length of the copy.
    static func stage(_ batch: UploadBatch) async throws -> (batch: UploadBatch, directory: URL) {
        try await Task.detached(priority: .userInitiated) { try stageOnDisk(batch) }.value
    }

    private static func stageOnDisk(
        _ batch: UploadBatch
    ) throws -> (batch: UploadBatch, directory: URL) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("omnibus-uploads/\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true
        )
        var staged: [URL] = []
        for (index, source) in batch.urls.enumerated() {
            let scoped = source.startAccessingSecurityScopedResource()
            defer { if scoped { source.stopAccessingSecurityScopedResource() } }
            // Two parts picked from different folders can share a name, and
            // the copy — like the server's own filing — would collide.
            var name = source.lastPathComponent
            var destination = directory.appendingPathComponent(name)
            if FileManager.default.fileExists(atPath: destination.path) {
                let ext = (name as NSString).pathExtension
                let stem = (name as NSString).deletingPathExtension
                name = ext.isEmpty ? "\(stem)-\(index)" : "\(stem)-\(index).\(ext)"
                destination = directory.appendingPathComponent(name)
            }
            try FileManager.default.copyItem(at: source, to: destination)
            staged.append(destination)
        }
        return (UploadBatch(kind: batch.kind, urls: staged), directory)
    }

    /// Delete a staged copy once its upload has finished, succeeded or not.
    static func discard(_ directory: URL) {
        try? FileManager.default.removeItem(at: directory)
    }

    /// Parse the picked file(s) and return the metadata to pre-fill the confirm
    /// form with. Nothing is written — the server discards its tempfile.
    static func inspect(_ batch: UploadBatch) async throws -> UploadConfirmation {
        let files = try await loadParts(batch)
        switch batch.kind {
        case .ebook:
            let inspection: UploadInspection = try await APIClient.shared.upload(
                batch.kind.inspectPath, files: files
            )
            return UploadConfirmation(
                title: inspection.title ?? "",
                author: inspection.author ?? "",
                series: inspection.series ?? "",
                seriesIndex: inspection.seriesIndex ?? ""
            )
        case .audiobook:
            let inspection: AudiobookInspection = try await APIClient.shared.upload(
                batch.kind.inspectPath, files: files
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
        let files = try await loadParts(batch)
        let fields = UploadFlow.commitFields(
            kind: batch.kind,
            title: confirmation.title,
            author: confirmation.author,
            series: confirmation.series,
            seriesIndex: confirmation.seriesIndex
        )
        let result: UploadCommitResult = try await APIClient.shared.upload(
            batch.kind.commitPath, files: files, fields: fields
        )
        // The library listing and its pages now predate a book that exists.
        await OfflineStore.shared.cacheDelete(CacheKey.library)
        await OfflineStore.shared.cacheDeletePrefix("books_page:")
        return result
    }

    /// Read every staged part off disk, off the main thread for the same
    /// reason [`stage`] copies there.
    private static func loadParts(_ batch: UploadBatch) async throws -> [MultipartFile] {
        try await Task.detached(priority: .userInitiated) {
            try batch.urls.map(loadPart)
        }.value
    }

    /// Read one staged part off disk. By the time either step runs the files
    /// live in this app's temporary directory — [`stage`] copied them out of
    /// the picker's security-scoped location — so no scope is claimed here and
    /// a slow confirm step cannot outlive one.
    private static func loadPart(_ url: URL) throws -> MultipartFile {
        let name = url.lastPathComponent
        return MultipartFile(
            fileName: name,
            mimeType: UploadFlow.mimeType(for: name),
            data: try Data(contentsOf: url)
        )
    }
}
