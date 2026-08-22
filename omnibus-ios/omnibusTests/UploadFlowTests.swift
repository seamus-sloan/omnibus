//  UploadFlowTests.swift
//  The upload's pure request shaping: which ingest a file targets, how a
//  multi-file pick groups into commits, and — the regression this file exists
//  for — that a commit body actually carries `title` and `author`. Sending the
//  file alone is what made every upload from the app 400.

import Foundation
import Testing
import UniformTypeIdentifiers

@testable import omnibus

private func url(_ name: String) -> URL {
    URL(fileURLWithPath: "/tmp/picked/\(name)")
}

struct UploadFlowTests {
    // MARK: - Format routing

    @Test func kindRoutesEpubToTheEbookIngest() {
        #expect(UploadFlow.kind(for: "Dune.epub") == .ebook)
        #expect(UploadFlow.kind(for: "DUNE.EPUB") == .ebook)
    }

    @Test func kindRoutesEveryAcceptedAudioContainerToTheAudiobookIngest() {
        #expect(UploadFlow.kind(for: "book.m4b") == .audiobook)
        #expect(UploadFlow.kind(for: "book.m4a") == .audiobook)
        #expect(UploadFlow.kind(for: "part-01.mp3") == .audiobook)
    }

    @Test func kindRejectsPlayableFormatsTheUploadEndpointsRefuse() {
        // `Book.audioFormats` covers what the library can play; routing on it
        // sent whole .flac/.wav files that the server then answered 415.
        for name in ["song.flac", "song.wav", "song.ogg", "song.opus", "song.aac"] {
            #expect(UploadFlow.kind(for: name) == nil)
        }
        #expect(UploadFlow.kind(for: "comic.cbz") == nil)
        #expect(UploadFlow.kind(for: "notes.pdf") == nil)
        #expect(UploadFlow.kind(for: "noextension") == nil)
    }

    @Test func pickerOffersOnlyFormatsTheServerAccepts() {
        let extensions = Set(
            UploadFlow.pickerTypes.flatMap { $0.tags[.filenameExtension] ?? [] }
        )
        #expect(extensions.contains("epub"))
        #expect(extensions.contains("mp3"))
        #expect(extensions.contains("m4a"))
        #expect(extensions.contains("m4b"))
        #expect(!extensions.contains("flac"))
        #expect(!extensions.contains("wav"))
    }

    @Test func mimeTypeMatchesTheContainerRatherThanGuessingMp4() {
        #expect(UploadFlow.mimeType(for: "a.epub") == "application/epub+zip")
        #expect(UploadFlow.mimeType(for: "a.mp3") == "audio/mpeg")
        #expect(UploadFlow.mimeType(for: "a.m4b") == "audio/mp4")
        #expect(UploadFlow.mimeType(for: "a.m4a") == "audio/mp4")
    }

    // MARK: - Grouping

    @Test func selectionGivesEachEpubItsOwnCommit() {
        let selection = UploadFlow.selection(for: [url("a.epub"), url("b.epub")])
        #expect(selection.batches.count == 2)
        #expect(selection.batches.allSatisfy { $0.kind == .ebook })
        #expect(selection.batches.allSatisfy { $0.urls.count == 1 })
        #expect(selection.unsupported.isEmpty)
    }

    @Test func selectionGroupsEveryMp3IntoOneMultiPartAudiobook() {
        // `classify_audio_set` files a set of .mp3 parts as a single book; one
        // request per file made an N-part audiobook into N one-chapter books.
        let picked = [url("01.mp3"), url("02.mp3"), url("03.mp3")]
        let selection = UploadFlow.selection(for: picked)
        #expect(selection.batches.count == 1)
        #expect(selection.batches.first?.kind == .audiobook)
        #expect(selection.batches.first?.urls == picked)
    }

    @Test func selectionGivesEachContainerItsOwnCommit() {
        // Two .m4b in one request is the server's MixedAudioUpload 400 —
        // each container is its own book, so each gets its own commit.
        let selection = UploadFlow.selection(for: [url("one.m4b"), url("two.m4a")])
        #expect(selection.batches.count == 2)
        #expect(selection.batches.allSatisfy { $0.urls.count == 1 })
    }

    @Test func selectionSplitsAMixedPickAndPutsTheMp3SetLast() {
        let selection = UploadFlow.selection(
            for: [url("a.epub"), url("01.mp3"), url("b.epub"), url("02.mp3")]
        )
        #expect(selection.batches.count == 3)
        #expect(selection.batches[0].urls == [url("a.epub")])
        #expect(selection.batches[1].urls == [url("b.epub")])
        // The MP3 set lands last however early its first part was picked — it
        // isn't complete until the whole selection has been walked.
        #expect(selection.batches[2].urls == [url("01.mp3"), url("02.mp3")])
    }

    @Test func selectionNamesUnsupportedFilesInsteadOfUploadingThem() {
        let selection = UploadFlow.selection(for: [url("a.epub"), url("song.flac")])
        #expect(selection.batches.count == 1)
        #expect(selection.unsupported == ["song.flac"])
    }

    @Test func displayNameIsTheFilenameForOneFileAndAPartCountForMany() {
        #expect(UploadBatch(kind: .ebook, urls: [url("Dune.epub")]).displayName == "Dune.epub")
        #expect(
            UploadBatch(kind: .audiobook, urls: [url("01.mp3"), url("02.mp3")]).displayName
                == "2 parts"
        )
    }

    // MARK: - Commit fields

    @Test func commitFieldsAlwaysCarryTitleAndAuthor() {
        let fields = UploadFlow.commitFields(
            kind: .ebook, title: "Dune", author: "Frank Herbert", series: "", seriesIndex: ""
        )
        #expect(fields.map(\.name) == ["title", "author"])
        #expect(fields.map(\.value) == ["Dune", "Frank Herbert"])
    }

    @Test func commitFieldsTrimAndDropBlankOptionals() {
        let fields = UploadFlow.commitFields(
            kind: .ebook, title: "  Dune  ", author: " Herbert ", series: "  ", seriesIndex: "\n"
        )
        #expect(fields.map(\.name) == ["title", "author"])
        #expect(fields.map(\.value) == ["Dune", "Herbert"])
    }

    @Test func commitFieldsIncludeSeriesForEbooksOnly() {
        let ebook = UploadFlow.commitFields(
            kind: .ebook, title: "Dune", author: "Herbert", series: "Dune", seriesIndex: "1"
        )
        #expect(ebook.map(\.name) == ["title", "author", "series", "series_index"])

        // The audiobook commit handler reads title/author and nothing else.
        let audiobook = UploadFlow.commitFields(
            kind: .audiobook, title: "Dune", author: "Herbert", series: "Dune", seriesIndex: "1"
        )
        #expect(audiobook.map(\.name) == ["title", "author"])
    }

    @Test func canCommitRequiresBothFields() {
        #expect(UploadFlow.canCommit(title: "Dune", author: "Herbert"))
        #expect(!UploadFlow.canCommit(title: "", author: "Herbert"))
        #expect(!UploadFlow.canCommit(title: "Dune", author: "   "))
    }

    // MARK: - Endpoints

    @Test func kindsPointAtTheirOwnInspectAndCommitRoutes() {
        #expect(UploadKind.ebook.inspectPath == "/api/uploads/ebooks/inspect")
        #expect(UploadKind.ebook.commitPath == "/api/uploads/ebooks")
        #expect(UploadKind.audiobook.inspectPath == "/api/uploads/audiobooks/inspect")
        #expect(UploadKind.audiobook.commitPath == "/api/uploads/audiobooks")
    }
}
