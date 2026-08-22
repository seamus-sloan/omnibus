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

        let (staged, directory) = try await UploadService.stage(batch)
        defer { UploadService.discard(directory) }

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

        let (staged, directory) = try await UploadService.stage(batch)
        defer { UploadService.discard(directory) }

        #expect(try Data(contentsOf: staged.urls[0]) == Data("PK:01.mp3".utf8))
        #expect(try Data(contentsOf: staged.urls[1]) == Data("PK:02.mp3".utf8))
    }

    @Test func stagePreservesFilenamesWhenTwoPartsCollide() async throws {
        // Two folders, same filename — legitimate for a multi-part audiobook
        // split across discs. Each part gets its own numbered slot, so the
        // ORIGINAL name goes out on the wire and the server's own
        // `dedupe_part_name` resolves the clash.
        //
        // The previous scheme renamed client-side to `track01-1.mp3`, which was
        // worse than useless: '-' (0x2D) sorts before '.' (0x2E), and
        // `build_parts_list` breaks ties on filename, so two discs both tagged
        // 1..N interleaved on playback.
        let discOne = try makeSourceDir(["track01.mp3"], contents: "ONE")
        let discTwo = try makeSourceDir(["track01.mp3"], contents: "TWO")
        defer {
            try? FileManager.default.removeItem(at: discOne)
            try? FileManager.default.removeItem(at: discTwo)
        }
        let batch = UploadBatch(
            kind: .audiobook,
            urls: [
                discOne.appendingPathComponent("track01.mp3"),
                discTwo.appendingPathComponent("track01.mp3"),
            ]
        )

        let (staged, directory) = try await UploadService.stage(batch)
        defer { UploadService.discard(directory) }

        #expect(staged.urls.count == 2)
        // Both keep the name the reader picked; no client-side rename can
        // reorder what the server sorts.
        #expect(staged.urls.map(\.lastPathComponent) == ["track01.mp3", "track01.mp3"])
        #expect(Set(staged.urls.map(\.path)).count == 2, "distinct paths, same filename")
        #expect(try Data(contentsOf: staged.urls[0]) == Data("ONE:track01.mp3".utf8))
        #expect(try Data(contentsOf: staged.urls[1]) == Data("TWO:track01.mp3".utf8))
    }

    @Test func stageSurvivesAThirdPartCollidingWithADisambiguatedName() async throws {
        // The old single-shot rename produced "track-2.mp3" for the third part
        // and threw, because the second part was already called that.
        let one = try makeSourceDir(["track.mp3", "track-2.mp3"], contents: "ONE")
        let two = try makeSourceDir(["track.mp3"], contents: "TWO")
        defer {
            try? FileManager.default.removeItem(at: one)
            try? FileManager.default.removeItem(at: two)
        }
        let batch = UploadBatch(
            kind: .audiobook,
            urls: [
                one.appendingPathComponent("track.mp3"),
                one.appendingPathComponent("track-2.mp3"),
                two.appendingPathComponent("track.mp3"),
            ]
        )

        let (staged, directory) = try await UploadService.stage(batch)
        defer { UploadService.discard(directory) }

        #expect(staged.urls.count == 3)
        #expect(
            staged.urls.map(\.lastPathComponent) == ["track.mp3", "track-2.mp3", "track.mp3"]
        )
        #expect(Set(staged.urls.map(\.path)).count == 3)
    }

    @Test func stageGivesEachBatchItsOwnDirectory() async throws {
        let source = try makeSourceDir(["a.epub"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(kind: .ebook, urls: [source.appendingPathComponent("a.epub")])

        let (_, first) = try await UploadService.stage(batch)
        let (_, second) = try await UploadService.stage(batch)
        defer {
            UploadService.discard(first)
            UploadService.discard(second)
        }

        // Per-batch, so discarding one finished upload cannot delete the files
        // of another still waiting on its confirm sheet.
        #expect(first != second)
    }

    @Test func discardRemovesTheStagedCopy() async throws {
        let source = try makeSourceDir(["a.epub"])
        defer { try? FileManager.default.removeItem(at: source) }
        let batch = UploadBatch(kind: .ebook, urls: [source.appendingPathComponent("a.epub")])

        let (staged, directory) = try await UploadService.stage(batch)
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

        await #expect(throws: (any Error).self) {
            try await UploadService.stage(batch)
        }
    }
}
