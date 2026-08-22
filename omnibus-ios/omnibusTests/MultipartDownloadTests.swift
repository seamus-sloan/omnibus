//  MultipartDownloadTests.swift
//  How a download describes the files it is made of, and how the player
//  decides a local copy is the whole book.
//
//  A multi-part audiobook is one book across several files, and every step
//  that forgets that has the same symptom: an item that physically ends at
//  part 1 while the timeline shows the whole book, so a resume past that
//  point clamps backwards, fires played-to-end, and marks the book finished.

import Foundation
import Testing

@testable import omnibus

private func audiobook(
    uuid: String = "u-1", format: String = "MP3", etag: String? = "e-1"
) -> Book {
    Book(
        id: 1,
        filename: "Red Rising",
        title: "Red Rising",
        uniqueIdentifier: uuid,
        formats: [format.lowercased()],
        bookFiles: [
            BookFileInfo(id: 9, format: format, filename: "Red Rising", ordinal: 0, etag: etag)
        ]
    )
}

private func epub(uuid: String = "u-2") -> Book {
    Book(
        id: 2,
        filename: "Babel",
        title: "Babel",
        uniqueIdentifier: uuid,
        formats: ["epub"],
        bookFiles: [BookFileInfo(id: 3, format: "EPUB", filename: "Babel.epub", ordinal: 0)]
    )
}

private func part(_ ordinal: Int64, mime: String = "audio/mpeg") -> ManifestPart {
    ManifestPart(
        ordinal: ordinal,
        url: "/api/audiobooks/u-1/parts/\(ordinal)",
        durationSeconds: 7_588.78,
        mime: mime
    )
}

private func direct(_ ordinals: [Int64], mime: String = "audio/mpeg") -> AudiobookManifest {
    .direct(
        parts: ordinals.map { part($0, mime: mime) },
        totalDuration: Double(ordinals.count) * 7_588.78,
        chapters: []
    )
}

private func file(_ ordinal: Int64, name: String, received: Int64 = 0, total: Int64 = 0, done: Bool = false)
    -> DownloadFile
{
    DownloadFile(
        ordinal: ordinal, urlPath: "/api/audiobooks/u-1/download?part=\(ordinal)",
        name: name, receivedBytes: received, totalBytes: total, done: done
    )
}

private func record(files: [DownloadFile], state: DownloadRecord.State = .complete) -> DownloadRecord {
    DownloadRecord(
        bookUUID: "u-1", kind: .audio, format: "mp3", state: state,
        files: files, updatedAt: 0, error: nil, sourceEtag: "e-1"
    )
}

@Suite("Multi-part audiobook downloads")
struct MultipartDownloadTests {
    // MARK: - Planning

    @Test("a multi-part audiobook plans one file per part")
    func planCoversEveryPart() throws {
        let plan = try DownloadManager.plan(
            book: audiobook(), kind: .audio, manifest: direct([0, 1, 2, 3])
        )
        #expect(plan.count == 4)
        #expect(plan.map(\.ordinal) == [0, 1, 2, 3])
        #expect(plan.map(\.name) == [
            "u-1.audio.0.mp3", "u-1.audio.1.mp3", "u-1.audio.2.mp3", "u-1.audio.3.mp3",
        ])
    }

    /// The gate is the point: `/parts/{ordinal}` is the playback stream and
    /// stays open to a `can_download = 0` reader, so planning against it
    /// would hand that reader a permanent copy.
    @Test("every planned part is fetched through the gated download route")
    func planUsesTheGatedDownloadRoute() throws {
        let plan = try DownloadManager.plan(
            book: audiobook(), kind: .audio, manifest: direct([0, 1])
        )
        #expect(plan.map(\.urlPath) == [
            "/api/audiobooks/u-1/download?part=0",
            "/api/audiobooks/u-1/download?part=1",
        ])
    }

    /// AC3: the single-file m4b case keeps both the URL and the on-disk name
    /// it had, so a copy already on the device is still the file the plan
    /// names.
    @Test("a single-part audiobook keeps the pre-multi-part URL and name")
    func planKeepsSingleFileShape() throws {
        let plan = try DownloadManager.plan(
            book: audiobook(format: "M4B"), kind: .audio, manifest: direct([0], mime: "audio/mp4")
        )
        #expect(plan.count == 1)
        #expect(plan[0].urlPath == "/api/audiobooks/u-1/download")
        #expect(plan[0].name == "u-1.audio.m4b")
    }

    @Test("an HLS-only audiobook cannot be planned at all")
    func planRefusesHLS() {
        #expect(throws: DownloadPlanError.self) {
            try DownloadManager.plan(
                book: audiobook(), kind: .audio, manifest: .hls(playlistURL: "/p.m3u8")
            )
        }
    }

    /// A manifest that never arrived is not an excuse to plan from the
    /// download URL alone — that is exactly the plan that called a four-part
    /// book complete after one part.
    @Test("an audiobook with no manifest cannot be planned")
    func planRefusesAMissingManifest() {
        #expect(throws: DownloadPlanError.self) {
            try DownloadManager.plan(book: audiobook(), kind: .audio, manifest: nil)
        }
    }

    @Test("an ebook plans the one file the /file route serves")
    func planEbook() throws {
        let plan = try DownloadManager.plan(book: epub(), kind: .ebook, manifest: nil)
        #expect(plan.count == 1)
        #expect(plan[0].urlPath == "/api/ebooks/u-2/file")
        #expect(plan[0].name == "u-2.ebook.epub")
    }

    @Test("part mime types name the file on disk")
    func mimeExtensions() {
        #expect(DownloadManager.mimeExtension("audio/mpeg") == "mp3")
        #expect(DownloadManager.mimeExtension("audio/mp4") == "m4b")
        #expect(DownloadManager.mimeExtension("audio/x-m4a") == "m4a")
        #expect(DownloadManager.mimeExtension("audio/aac") == "aac")
        #expect(DownloadManager.mimeExtension("application/octet-stream") == "m4b")
    }

    // MARK: - The record

    /// AC2, one layer down: `local_path` is what a build predating multi-part
    /// downloads reads. Left pointing at part 1 it would play part 1 as the
    /// book; left null that build streams instead.
    @Test("a multi-file record exposes no single local path")
    func multiFileRecordHasNoSinglePath() {
        let multi = record(files: [file(0, name: "a.mp3"), file(1, name: "b.mp3")])
        #expect(multi.localPath == nil)

        let single = record(files: [file(0, name: "a.m4b")])
        #expect(single.localPath == "a.m4b")
    }

    @Test("byte counters aggregate across every file")
    func bytesAggregate() {
        let partial = record(
            files: [
                file(0, name: "a.mp3", received: 100, total: 100, done: true),
                file(1, name: "b.mp3", received: 50, total: 200),
            ],
            state: .running
        )
        #expect(partial.receivedBytes == 150)
        #expect(partial.totalBytes == 300)
    }

    /// Progress may not walk backwards. Summing bytes does exactly that: a
    /// part the system hasn't started contributes no size, so the denominator
    /// grows as parts begin and the bar repeatedly climbs to full and drops.
    @Test("progress only ever rises as parts start and finish")
    func fractionIsMonotonic() {
        // Four parts planned; only the first has been sized and half-fetched.
        let planned = record(
            files: [
                file(0, name: "a.mp3", received: 50, total: 100),
                file(1, name: "b.mp3"),
                file(2, name: "c.mp3"),
                file(3, name: "d.mp3"),
            ],
            state: .running
        )
        #expect(planned.fraction == 0.125)

        // The second part starts and is sized. Summing bytes would have read
        // 100% a moment ago and 25% now.
        let started = record(
            files: [
                file(0, name: "a.mp3", received: 100, total: 100, done: true),
                file(1, name: "b.mp3", received: 50, total: 100),
                file(2, name: "c.mp3"),
                file(3, name: "d.mp3"),
            ],
            state: .running
        )
        #expect(started.fraction > planned.fraction)
        #expect(started.fraction == 0.375)
    }

    // MARK: - Records written by the build this replaces

    /// The previous build filled `local_path` only on completion, so a
    /// download still in flight when the app updated has neither column. Its
    /// name is still derivable, and has to be: without a file to attribute it
    /// to, the completion the background session was still carrying is thrown
    /// away as a failure on a perfectly good 200.
    @Test("an in-flight legacy row still names the file it is fetching")
    func legacyInFlightRowIsNameable() {
        // The pre-upgrade row: no `files` JSON, and no `local_path` either,
        // because that build only wrote one on completion.
        let files = OfflineStore.readFiles(
            json: nil, legacyPath: nil, uuid: "u-1", kind: .audio, format: "m4b",
            totalBytes: 900, receivedBytes: 400, state: .running
        )
        #expect(files.count == 1)
        #expect(files[0].name == "u-1.audio.m4b")
        #expect(files[0].done == false)
        #expect(files[0].receivedBytes == 400)
    }

    /// A completed row keeps naming the file already on disk, and a path
    /// stored under a container UUID iOS has since reassigned still resolves.
    @Test("a completed legacy row resolves to the file it left on disk")
    func legacyCompleteRowResolves() {
        let files = OfflineStore.readFiles(
            json: nil, legacyPath: "/old/container/u-1.ebook.epub",
            uuid: "u-1", kind: .ebook, format: "epub",
            totalBytes: 500, receivedBytes: 500, state: .complete
        )
        #expect(files.count == 1)
        #expect(files[0].name == "u-1.ebook.epub")
        #expect(files[0].done)
    }

    /// A completed row reconstructed from `local_path` is the shape the
    /// partial-download heal keys on, and a row planned by this build is not.
    @Test("only a reconstructed record reads as the legacy shape")
    func legacyShapeIsDetectable() {
        #expect(record(files: [
            DownloadFile(ordinal: 0, urlPath: "", name: "u-1.audio.m4b", done: true)
        ]).isLegacyShape)
        #expect(!record(files: [file(0, name: "u-1.audio.m4b", done: true)]).isLegacyShape)
    }

    /// The incident in #2103 is a record exactly like this one: a completed
    /// download holding part 1 of a four-part book. It must not read as a
    /// usable copy.
    @Test("a legacy record holding one part of a multi-part book is not coverage")
    func legacyPartialIsNotCoverage() {
        let legacy = record(files: [
            DownloadFile(ordinal: 0, urlPath: "", name: "u-1.audio.mp3", done: true)
        ])
        #expect(legacy.isLegacyShape)
        #expect(!AudioPlayer.localCovers(manifest: direct([0, 1, 2, 3]), localFileCount: legacy.files.count))
    }

    @Test("a task key names the record and the part it is fetching")
    func taskKeysRoundTrip() {
        let key = DownloadRecord.key("u-1", .audio)
        let taskKey = DownloadRecord.taskKey(key, ordinal: 3)
        #expect(taskKey == "u-1:audio#3")
        #expect(DownloadRecord.recordID(fromTaskKey: taskKey) == key)
        #expect(DownloadRecord.ordinal(fromTaskKey: taskKey) == 3)
    }

    /// A transfer handed to the system by a build that predates per-part
    /// tasks carries the bare record key, and its completion still has to
    /// find its file after an upgrade.
    @Test("a task key without a part still names its record")
    func legacyTaskKeyIsTolerated() {
        #expect(DownloadRecord.recordID(fromTaskKey: "u-1:audio") == "u-1:audio")
        #expect(DownloadRecord.ordinal(fromTaskKey: "u-1:audio") == nil)

        let single = record(files: [file(0, name: "a.m4b")])
        #expect(single.index(ofOrdinal: nil) == 0)
    }

    @Test("a part ordinal resolves to its own file, not its position")
    func ordinalsResolveByValue() {
        let multi = record(files: [file(2, name: "c.mp3"), file(5, name: "f.mp3")])
        #expect(multi.index(ofOrdinal: 5) == 1)
        #expect(multi.index(ofOrdinal: 2) == 0)
        #expect(multi.index(ofOrdinal: 9) == nil)
    }

    @Test("nothing is installed under the name it will finally take")
    func incomingNamesAreDistinct() {
        let file = file(1, name: "u-1.audio.1.mp3")
        #expect(file.incomingName != file.name)
        #expect(file.incomingName.hasSuffix(file.name))
    }

    // MARK: - What the player will play from disk

    /// AC1/AC2: the count check. A four-part book with one file on the device
    /// must not be played from that file — the item would end at part 1's
    /// duration while the timeline still claimed the whole book.
    @Test("a local copy is used only when it covers every part")
    func localCoverageRequiresEveryPart() {
        #expect(AudioPlayer.localCovers(manifest: direct([0, 1, 2, 3]), localFileCount: 4))
        #expect(!AudioPlayer.localCovers(manifest: direct([0, 1, 2, 3]), localFileCount: 1))
        #expect(!AudioPlayer.localCovers(manifest: direct([0, 1, 2, 3]), localFileCount: 3))
        #expect(!AudioPlayer.localCovers(manifest: direct([0, 1]), localFileCount: 0))
    }

    /// AC3: a single-file audiobook goes on playing from disk exactly as it
    /// did before.
    @Test("a single-file audiobook still plays from its downloaded copy")
    func singleFileStillPlaysLocally() {
        #expect(AudioPlayer.localCovers(manifest: direct([0]), localFileCount: 1))
    }

    /// An HLS book names no parts, so there is nothing to check a local copy
    /// against — and the server has to transcode it as it plays anyway.
    @Test("an HLS book is never played from a local copy")
    func hlsNeverPlaysLocally() {
        #expect(!AudioPlayer.localCovers(
            manifest: .hls(playlistURL: "/p.m3u8"), localFileCount: 1
        ))
    }
}
