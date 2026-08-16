//  CrossFormatTests.swift
//  Codecs for the cross-format sync wire types plus the pure prompt rules
//  (re-arm clock, file-match gate).

import Foundation
import Testing
@testable import omnibus

@Suite("Cross-format codecs")
struct CrossFormatCodecTests {
    @Test("resume candidate decodes the wire shape")
    func candidateDecodes() throws {
        let json = """
        {"state":"candidate","candidate":{"target":"audio","source_format":"epub",
         "source_client_updated_at":1755000000,"confidence":"chapter_anchored",
         "book_file_id":7,"audio_position_seconds":312.5,"total_duration_seconds":600.0}}
        """
        let resume = try JSONDecoder().decode(CrossFormatResume.self, from: Data(json.utf8))
        #expect(resume.state == .candidate)
        let c = try #require(resume.candidate)
        #expect(c.target == .audio)
        #expect(c.sourceFormat == .epub)
        #expect(c.confidence == .chapterAnchored)
        #expect(c.bookFileID == 7)
        #expect(c.audioPositionSeconds == 312.5)
        #expect(c.percent == nil)
        #expect(c.fraction == nil)
    }

    @Test("epub-target candidate carries the full-precision fraction")
    func fractionDecodes() throws {
        let json = """
        {"state":"candidate","candidate":{"target":"epub","source_format":"audio",
         "source_client_updated_at":1755000000,"confidence":"linear",
         "percent":12,"fraction":0.12752}}
        """
        let resume = try JSONDecoder().decode(CrossFormatResume.self, from: Data(json.utf8))
        let c = try #require(resume.candidate)
        #expect(c.percent == 12)
        #expect(c.fraction == 0.12752)
    }

    @Test("follow mode and link anchors decode from the wire")
    func followDecodes() throws {
        let json = """
        {"state":"candidate","follow":true,"candidate":{"target":"epub",
         "source_format":"audio","source_client_updated_at":1,"confidence":"user_anchored",
         "percent":40,"fraction":0.405}}
        """
        let resume = try JSONDecoder().decode(CrossFormatResume.self, from: Data(json.utf8))
        #expect(resume.follow == true)
        #expect(resume.candidate?.confidence == .userAnchored)

        let linkJSON = """
        {"mode":"sequence","stale":false,"confirmed_at":9,"follow":true,"user_anchors":2}
        """
        let link = try JSONDecoder().decode(AlignmentLink.self, from: Data(linkJSON.utf8))
        #expect(link.follow == true)
        #expect(link.userAnchors == 2)
    }

    @Test("sync point declaration encodes snake_case keys")
    func declarationEncodes() throws {
        let decl = DeclareSyncPoint(
            bookUUID: "u-1",
            format: .audio,
            ebookFraction: nil,
            audioBookFileID: 7,
            audioSeconds: 123.5
        )
        let data = try JSONEncoder().encode(decl)
        let obj = try #require(
            try JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        #expect(obj["book_uuid"] as? String == "u-1")
        #expect(obj["audio_book_file_id"] as? Int == 7)
        #expect(obj["audio_seconds"] as? Double == 123.5)
    }

    @Test("candidate-less states decode without a candidate")
    func statesDecode() throws {
        for (raw, state) in [
            ("not_linked", CrossFormatResumeState.notLinked),
            ("link_stale", .linkStale),
            ("nothing_newer", .nothingNewer),
        ] {
            let resume = try JSONDecoder().decode(
                CrossFormatResume.self,
                from: Data("{\"state\":\"\(raw)\"}".utf8)
            )
            #expect(resume.state == state)
            #expect(resume.candidate == nil)
        }
    }

    @Test("alignment view decodes lanes, link, and match")
    func alignmentDecodes() throws {
        let json = """
        {"link":{"mode":"narrations","primary_book_file_id":3,"stale":false,"confirmed_at":1},
         "anchor_match":{"matched":12,"ebook_chapters":14,"confidence":"chapter_anchored"},
         "ebook":{"total_chars":1000,"chapters":[{"title":"One","percent":0.0}]},
         "audio_files":[{"book_file_id":3,"label":"a.m4b","duration_seconds":600.0,
                         "chapter_starts":[0.0,300.0]}],
         "reading":{"percent":40,"client_updated_at":5},
         "listening":{"seconds":120.0,"client_updated_at":4}}
        """
        let view = try JSONDecoder().decode(AlignmentView.self, from: Data(json.utf8))
        #expect(view.link?.mode == .narrations)
        #expect(view.anchorMatch?.matched == 12)
        #expect(view.ebook?.chapters.first?.title == "One")
        #expect(view.audioFiles.first?.chapterStarts.count == 2)
        #expect(view.reading?.percent == 40)
        #expect(view.listening?.bookFileID == nil)
    }
}

@Suite("Sync prompt rules")
struct SyncPromptRuleTests {
    @Test("a declined clock stays quiet until the source advances")
    func rearmRule() {
        #expect(SyncPromptStore.shouldOffer(sourceClock: 10, dismissed: nil))
        #expect(!SyncPromptStore.shouldOffer(sourceClock: 10, dismissed: 10))
        #expect(!SyncPromptStore.shouldOffer(sourceClock: 9, dismissed: 10))
        #expect(SyncPromptStore.shouldOffer(sourceClock: 11, dismissed: 10))
    }

    @Test("the file gate refuses ambiguity, never guesses")
    func fileGate() {
        // Explicit selection must match exactly.
        #expect(SyncPromptStore.fileMatches(selected: 7, defaultID: 1, candidate: 7))
        #expect(!SyncPromptStore.fileMatches(selected: 7, defaultID: 1, candidate: 8))
        // No selection: the default (first-by-ordinal) file is loaded.
        #expect(SyncPromptStore.fileMatches(selected: nil, defaultID: 7, candidate: 7))
        #expect(!SyncPromptStore.fileMatches(selected: nil, defaultID: 1, candidate: 7))
        // A candidate with no file is never seekable here.
        #expect(!SyncPromptStore.fileMatches(selected: nil, defaultID: nil, candidate: nil))
    }
}
