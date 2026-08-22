//  UploadStagingTests.swift
//  Staging picked files into the app's temporary directory — the step that
//  buys the confirm sheet its unbounded human pause without holding a
//  security scope or a resident `Data`.
//
//  The collision branch is the reason this file exists: two parts of one
//  audiobook picked from different folders can share a filename, and the copy
//  would otherwise throw rather than upload.

import Foundation
import Testing

@testable import omnibus

/// A throwaway directory holding real files to pick from.
private func makeSourceDir(_ names: [String], contents: String = "PK") throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("upload-staging-tests/\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    for name in names {
        try Data("\(contents):\(name)".utf8)
            .write(to: dir.appendingPathComponent(name))
    }
    return dir
}

struct UploadStagingTests {
    @Test func stageCopiesEveryPartAndLeavesTheOriginalsAlone() async throws {
        let source = try makeSourceDir(["01.mp3", "02.mp3"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(
            kind: .audiobook,
            urls: ["01.mp3", "02.mp3"].map { source.appendingPathComponent($0) }
        )

        let directory = UploadService.makeStagingDirectory()
        defer { UploadService.discardNow(directory) }
        let staged = try await UploadService.stage(batch, into: directory)

        #expect(staged.kind == .audiobook)
        #expect(staged.urls.count == 2)
        #expect(staged.urls.map(\.lastPathComponent) == ["01.mp3", "02.mp3"])
        // Rebased onto the copies, not still pointing at the picker's URLs.
        #expect(staged.urls.allSatisfy { $0.path.hasPrefix(directory.path) })
        for url in staged.urls {
            #expect(FileManager.default.fileExists(atPath: url.path))
        }
        // The originals survive — staging copies, it does not move.
        #expect(
            FileManager.default.fileExists(
                atPath: source.appendingPathComponent("01.mp3").path
            )
        )
    }

    @Test func stagePreservesEachPartsBytes() async throws {
        let source = try makeSourceDir(["01.mp3", "02.mp3"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(
            kind: .audiobook,
            urls: ["01.mp3", "02.mp3"].map { source.appendingPathComponent($0) }
        )

        let directory = UploadService.makeStagingDirectory()
        defer { UploadService.discardNow(directory) }
        let staged = try await UploadService.stage(batch, into: directory)

        #expect(try Data(contentsOf: staged.urls[0]) == Data("PK:01.mp3".utf8))
        #expect(try Data(contentsOf: staged.urls[1]) == Data("PK:02.mp3".utf8))
    }

    @Test func stageKeepsPickedFilenamesForTheWire() async throws {
        // Per-part slots mean the ORIGINAL name is what the server sees, so no
        // client-side rename can reorder what `build_parts_list` sorts.
        let source = try makeSourceDir(["01.mp3", "02.mp3"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(
            kind: .audiobook,
            urls: ["01.mp3", "02.mp3"].map { source.appendingPathComponent($0) }
        )
        let directory = UploadService.makeStagingDirectory()
        defer { UploadService.discardNow(directory) }

        let staged = try await UploadService.stage(batch, into: directory)
        #expect(staged.urls.map(\.lastPathComponent) == ["01.mp3", "02.mp3"])
        #expect(Set(staged.urls.map(\.path)).count == 2)
    }

    @Test func stageGivesEachBatchItsOwnDirectory() async throws {
        let source = try makeSourceDir(["a.epub"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(kind: .ebook, urls: [source.appendingPathComponent("a.epub")])

        let first = UploadService.makeStagingDirectory()
        let second = UploadService.makeStagingDirectory()
        defer {
            UploadService.discardNow(first)
            UploadService.discardNow(second)
        }
        _ = try await UploadService.stage(batch, into: first)
        _ = try await UploadService.stage(batch, into: second)

        // Per-batch, so discarding one finished upload cannot delete the files
        // of another still waiting on its confirm sheet.
        #expect(first != second)
    }

    @Test func discardRemovesTheStagedCopy() async throws {
        let source = try makeSourceDir(["a.epub"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(kind: .ebook, urls: [source.appendingPathComponent("a.epub")])

        let directory = UploadService.makeStagingDirectory()
        let staged = try await UploadService.stage(batch, into: directory)
        // The synchronous core, so the assertion does not race the detached
        // task `discard` normally fires.
        UploadService.discardNow(directory)

        #expect(!FileManager.default.fileExists(atPath: staged.urls[0].path))
        #expect(!FileManager.default.fileExists(atPath: directory.path))
        // The picked original is untouched by discard.
        #expect(
            FileManager.default.fileExists(atPath: source.appendingPathComponent("a.epub").path)
        )
    }

    @Test func stageThrowsWhenAPickedFileIsGone() async throws {
        let source = try makeSourceDir([])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(
            kind: .ebook, urls: [source.appendingPathComponent("missing.epub")]
        )

        let directory = UploadService.makeStagingDirectory()
        defer { UploadService.discardNow(directory) }
        await #expect(throws: (any Error).self) {
            try await UploadService.stage(batch, into: directory)
        }
    }
}

/// The sweep's ownership filter, and the partial-copy cleanup it backstops.
///
/// These are the paths that kept regressing: cleanup that fired at the wrong
/// moment, or not at all. Each test sweeps its OWN root — the sweep is
/// destructive over a whole directory, and Swift Testing runs in parallel, so a
/// test pointed at the real staging root would delete another test's files.
struct UploadSweepTests {
    private func makeRoot() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("upload-sweep-tests/\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    private func makeStaged(_ name: String, under root: URL) throws -> URL {
        let dir = root.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: dir.appendingPathComponent("part.mp3"))
        return dir
    }

    @Test func sweepRemovesAbandonedStagingButKeepsLiveDirectories() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let live = try makeStaged(UUID().uuidString, under: root)
        let abandoned = try makeStaged(UUID().uuidString, under: root)

        UploadService.sweepStaging(keeping: [live], in: root)

        // The regression this pins: a blind sweep of the root deleted the parts
        // of an upload that outlived its sheet and was still reading them.
        #expect(FileManager.default.fileExists(atPath: live.path))
        #expect(!FileManager.default.fileExists(atPath: abandoned.path))
    }

    @Test func sweepMatchesLiveDirectoriesThroughAPathSymlink() throws {
        // `temporaryDirectory` hands back /var/... while `contentsOfDirectory`
        // returns /private/var/... . Comparing URLs (even standardized) misses
        // that and swept every live directory.
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let live = try makeStaged(UUID().uuidString, under: root)
        let aliased = URL(fileURLWithPath: "/private" + live.path)

        UploadService.sweepStaging(keeping: [aliased], in: root)
        #expect(FileManager.default.fileExists(atPath: live.path))
    }

    @Test func sweepWithNothingLiveClearsEverything() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let a = try makeStaged(UUID().uuidString, under: root)
        let b = try makeStaged(UUID().uuidString, under: root)

        UploadService.sweepStaging(keeping: [], in: root)

        #expect(!FileManager.default.fileExists(atPath: a.path))
        #expect(!FileManager.default.fileExists(atPath: b.path))
    }

    @Test func sweepReclaimsAPartiallyStagedDirectory() async throws {
        // A copy that throws part-way has already written the parts it got
        // through. The directory is allocated before the copy starts precisely
        // so it stays reclaimable — previously the handle existed only on
        // success, so a disk-full upload leaked what it had already written.
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let source = try makeStaged("source", under: root)
        try Data("ok".utf8).write(to: source.appendingPathComponent("01.mp3"))

        let directory = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let batch = UploadBatch(
            kind: .audiobook,
            urls: [
                source.appendingPathComponent("01.mp3"),
                source.appendingPathComponent("missing.mp3"),
            ]
        )
        await #expect(throws: (any Error).self) {
            try await UploadService.stage(batch, into: directory)
        }
        #expect(FileManager.default.fileExists(atPath: directory.path), "part 0 landed")

        UploadService.sweepStaging(keeping: [source], in: root)
        #expect(!FileManager.default.fileExists(atPath: directory.path))
    }

    @Test func staleErrorsAreNotTreatedAsReachingTheServer() {
        // A transport failure means the request never landed, so invalidating
        // the library — which deletes cached reads — would blank the shelf for
        // a reader who is now offline and cannot refill it.
        #expect(!UploadManager.mayHaveReachedTheServer(APIError.offline))
        #expect(!UploadManager.mayHaveReachedTheServer(APIError.transport("lost")))
        #expect(!UploadManager.mayHaveReachedTheServer(APIError.notConfigured))
        // A status code is proof the server processed the request — and it may
        // have filed the book before failing validation.
        #expect(UploadManager.mayHaveReachedTheServer(APIError.http(status: 400, message: "no")))
        #expect(UploadManager.mayHaveReachedTheServer(APIError.http(status: 408, message: "")))
    }
}
