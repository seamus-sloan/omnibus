//  DownloadValidatorSweepTests.swift
//  The batched staleness sweep: which downloads it asks about, how it splits
//  the ask, and what it is allowed to write down.
//
//  Rule 09 ("Asking about many files is one request") is what this covers from
//  the client side. The comparison behind it is three-valued, and the sweep is
//  the caller that *stores* the answer — so the case that must not regress is
//  an unanswerable read quietly clearing a flag a real comparison had set.

import Foundation
import Testing

@testable import omnibus

private func record(
    uuid: String = "u-1",
    kind: DownloadKind = .ebook,
    state: DownloadRecord.State = .complete,
    sourceEtag: String? = "\"old\"",
    stale: Bool? = nil
) -> DownloadRecord {
    DownloadRecord(
        bookUUID: uuid, kind: kind, format: kind == .audio ? "mp3" : "epub", state: state,
        files: [DownloadFile(ordinal: 0, urlPath: "/api/ebooks/\(uuid)/file", name: "\(uuid).epub")],
        updatedAt: 0, error: nil, sourceEtag: sourceEtag, stale: stale
    )
}

private func answer(
    uuid: String = "u-1", format: DownloadValidatorFormat = .epub, etag: String?
) -> DownloadValidator {
    DownloadValidator(bookUUID: uuid, format: format, etag: etag)
}

@Suite("Download validator sweep")
struct DownloadValidatorSweepTests {
    // MARK: - What gets asked

    @Test("only completed downloads are asked about")
    func onlyCompletedDownloadsAreAsked() {
        // An unfinished download has no local copy for a newer file to be
        // newer *than*, so there is nothing to flag and nothing to ask.
        let queries = DownloadManager.validatorQueries(for: [
            record(uuid: "done", state: .complete),
            record(uuid: "running", state: .running),
            record(uuid: "queued", state: .queued),
            record(uuid: "failed", state: .failed),
        ])
        #expect(queries.map(\.bookUUID) == ["done"])
    }

    @Test("each downloaded format asks about its own file")
    func eachFormatIsItsOwnQuery() {
        // Two downloads of one book are two files on disk and two questions;
        // the wire spells the ebook side `epub`, not the registry's `ebook`.
        let queries = DownloadManager.validatorQueries(for: [
            record(uuid: "u-1", kind: .audio),
            record(uuid: "u-1", kind: .ebook),
        ])
        #expect(queries.count == 2)
        #expect(queries.map(\.format) == [.audio, .epub])
        #expect(queries.allSatisfy { $0.bookUUID == "u-1" })
    }

    @Test("a query carries no file_id, so the server answers about the row it serves")
    func queriesOmitFileID() throws {
        // An iOS download never picks a `book_files` row, so the file the
        // server resolves by default is exactly the one it served. Sending a
        // `file_id` we invented would ask about a different file.
        let body = DownloadValidatorRequest(files: DownloadManager.validatorQueries(for: [record()]))
        let json = try #require(String(data: JSONEncoder().encode(body), encoding: .utf8))
        #expect(!json.contains("file_id"))
        #expect(json.contains("\"book_uuid\""))
        #expect(json.contains("\"epub\""))
    }

    // MARK: - The chunking boundary

    @Test("a device under the cap asks exactly once")
    func oneRequestUnderTheCap() {
        let queries = (0..<DownloadManager.maxValidatorQuery).map {
            DownloadValidatorQuery(bookUUID: "u-\($0)", format: .epub)
        }
        let chunks = DownloadManager.chunked(queries, size: DownloadManager.maxValidatorQuery)
        #expect(chunks.count == 1)
        #expect(chunks[0].count == DownloadManager.maxValidatorQuery)
    }

    @Test("one over the cap splits rather than being rejected")
    func splitsAboveTheCap() {
        // `post_download_validators` answers 422 to an over-cap body, so the
        // whole sweep would come back empty rather than one book's answer.
        let cap = DownloadManager.maxValidatorQuery
        let queries = (0...cap).map { DownloadValidatorQuery(bookUUID: "u-\($0)", format: .epub) }
        let chunks = DownloadManager.chunked(queries, size: cap)
        #expect(chunks.count == 2)
        #expect(chunks.map(\.count) == [cap, 1])
        #expect(chunks.flatMap { $0 } == queries)
    }

    @Test("a device with no downloads sends no request at all")
    func emptySweepAsksNothing() {
        #expect(DownloadManager.chunked([], size: DownloadManager.maxValidatorQuery).isEmpty)
        #expect(DownloadManager.validatorQueries(for: []).isEmpty)
    }

    // MARK: - Three-valued comparison

    @Test("a moved validator is what marks a download stale")
    func aMovedValidatorIsStale() {
        #expect(DownloadManager.staleness(snapshot: "\"old\"", current: "\"new\"") == true)
        #expect(DownloadManager.staleness(snapshot: "\"same\"", current: "\"same\"") == false)
    }

    @Test("a missing validator on either side is unanswerable, not fresh")
    func missingValidatorsAreUnanswerable() {
        #expect(DownloadManager.staleness(snapshot: nil, current: "\"new\"") == nil)
        #expect(DownloadManager.staleness(snapshot: "\"old\"", current: nil) == nil)
        #expect(DownloadManager.staleness(snapshot: nil, current: nil) == nil)
    }

    // MARK: - What the sweep may write down

    @Test("a real comparison is stored")
    func realComparisonsAreStored() {
        #expect(
            DownloadManager.staleUpdate(for: record(), from: answer(etag: "\"new\"")) == true
        )
        #expect(
            DownloadManager.staleUpdate(
                for: record(stale: true), from: answer(etag: "\"old\"")
            ) == false
        )
    }

    @Test("an unanswerable read never clears a flag a real comparison set")
    func cantTellLeavesTheFlagAlone() {
        // The whole reason `staleness` is three-valued. A book the server no
        // longer has, or a `book_files` row the scanner hasn't stat'd, comes
        // back with no etag — and this is the caller that *writes* the answer,
        // so collapsing that to "not stale" would drop the chip.
        #expect(DownloadManager.staleUpdate(for: record(stale: true), from: answer(etag: nil)) == nil)

        // Same on the other side: a record taken before validators existed
        // carries no snapshot to compare with.
        #expect(
            DownloadManager.staleUpdate(
                for: record(sourceEtag: nil, stale: true), from: answer(etag: "\"new\"")
            ) == nil
        )
    }

    @Test("an answer that agrees with the stored flag is not a write")
    func agreeingAnswersAreNotWrites() {
        #expect(
            DownloadManager.staleUpdate(
                for: record(stale: true), from: answer(etag: "\"new\"")
            ) == nil
        )
        #expect(
            DownloadManager.staleUpdate(
                for: record(stale: false), from: answer(etag: "\"old\"")
            ) == nil
        )
    }

    @Test("an unfinished download is never flagged")
    func unfinishedDownloadsAreNeverFlagged() {
        // Its bytes are still arriving; inviting a reader to replace them
        // would be inviting them to restart the download they are waiting on.
        #expect(
            DownloadManager.staleUpdate(
                for: record(state: .running), from: answer(etag: "\"new\"")
            ) == nil
        )
    }

    // MARK: - Wire shape

    @Test("the response decodes the server's wire vocabulary")
    func responseDecodes() throws {
        let json = """
            {"files":[
              {"book_uuid":"u-1","format":"epub","etag":"\\"a-b\\""},
              {"book_uuid":"u-2","format":"audio"}
            ]}
            """
        let decoded = try JSONDecoder().decode(
            DownloadValidatorResponse.self, from: Data(json.utf8)
        )
        #expect(decoded.files.count == 2)
        #expect(decoded.files[0] == DownloadValidator(bookUUID: "u-1", format: .epub, etag: "\"a-b\""))
        // An omitted etag is the "can't tell" the server sends for a file it
        // cannot answer about — it must decode, not throw.
        #expect(decoded.files[1].etag == nil)
        #expect(decoded.files[1].format.kind == .audio)
    }

    @Test("the wire format maps back to the registry's own kinds")
    func formatsRoundTripToKinds() {
        for kind in DownloadKind.allCases {
            #expect(DownloadValidatorFormat(kind).kind == kind)
        }
    }
}
