import Foundation
import Testing

@testable import omnibus

/// Decoding `GET /api/progress/{uuid}`.
///
/// These are wire-contract tests, not logic tests, and they exist because the
/// compiler cannot catch the failure they cover: a stored property missing from
/// `CodingKeys` still builds — an optional gets its implicit `nil` — and simply
/// never decodes. The whole point of the payload is the fields added here, so
/// silently dropping them would look exactly like the server not sending them.
@Suite("Book progress decoding")
struct BookProgressDecodeTests {
    private func decode(_ json: String) throws -> BookProgress {
        try JSONDecoder().decode(BookProgress.self, from: Data(json.utf8))
    }

    @Test("an audio record carries its runtime and resolved chapter")
    func audioRecordCarriesRuntimeAndChapter() throws {
        let progress = try decode(
            """
            {
              "book_uuid": "bk",
              "furthest": "audio",
              "linked": true,
              "records": [{
                "book_uuid": "bk",
                "format": "audio",
                "epub_cfi": null,
                "audio_position_seconds": 900.0,
                "progress_percent": 75,
                "updated_at": 100,
                "client_updated_at": 100,
                "total_duration_seconds": 1200.0,
                "resolved": {
                  "chapter_title": "Endings",
                  "chapter_ordinal": 3,
                  "chapters_total": 3,
                  "percent_through_chapter": 25,
                  "percent_through_book": 75,
                  "confidence": "high"
                }
              }]
            }
            """)

        #expect(progress.furthest == .audio)
        #expect(progress.linked)
        let record = try #require(progress.record(for: .audio))
        #expect(record.totalDurationSeconds == 1200.0)
        #expect(record.resolved?.chapterTitle == "Endings")
        #expect(record.resolved?.chapterOrdinal == 3)
        #expect(record.resolved?.percentThroughBook == 75)
        #expect(record.resolved?.confidence == .high)
    }

    @Test("both formats come back, and record(for:) picks one")
    func bothFormatsDecodeAndNarrow() throws {
        let progress = try decode(
            """
            {
              "book_uuid": "bk",
              "furthest": "audio",
              "records": [
                {"book_uuid": "bk", "format": "epub", "progress_percent": 47,
                 "updated_at": 200, "client_updated_at": 200},
                {"book_uuid": "bk", "format": "audio", "audio_position_seconds": 900.0,
                 "progress_percent": 87, "updated_at": 100, "client_updated_at": 100}
              ]
            }
            """)

        #expect(progress.records.count == 2)
        #expect(progress.record(for: .epub)?.progressPercent == 47)
        #expect(progress.record(for: .audio)?.progressPercent == 87)
        // The reader's true place is the audiobook, whichever they touched last.
        #expect(progress.furthest == .audio)
    }

    @Test("a book the reader has never opened decodes as an empty envelope")
    func unopenedBookDecodesEmpty() throws {
        let progress = try decode("""
            {"book_uuid": "bk", "records": [], "furthest": null}
            """)
        #expect(progress.records.isEmpty)
        #expect(progress.furthest == nil)
        #expect(!progress.linked)
        #expect(progress.record(for: .epub) == nil)
    }

    @Test("a low-confidence block is demoted to a part, never rendered as a chapter")
    func lowConfidenceResolvesToAPart() throws {
        let progress = try decode(
            """
            {
              "book_uuid": "bk", "furthest": "audio",
              "records": [{
                "book_uuid": "bk", "format": "audio", "audio_position_seconds": 900.0,
                "updated_at": 100, "client_updated_at": 100,
                "total_duration_seconds": 1200.0,
                "resolved": {"chapter_title": "Part 3", "chapter_ordinal": 3,
                             "chapters_total": 3, "confidence": "low"}
              }]
            }
            """)
        let record = try #require(progress.record(for: .audio))
        let point = ResumePoint(
            record: record, book: Book(id: 1, filename: "a.m4b"),
            audioPart: 3, audioPartCount: 3
        )
        #expect(point.structuralPosition == .part(ordinal: 3, total: 3))
    }
}
