//  UploadFlowTests.swift
//  The upload's pure request shaping: which ingest a file targets, how a
//  multi-file pick groups into commits, and — the regression this file exists
//  for — that a commit body actually carries `title` and `author`. Sending the
//  file alone is what made every upload from the app 400.

import Foundation
import Testing
import UniformTypeIdentifiers

@testable import omnibus

private func commits(
    title: String, author: String, series: String = "", index: String = "",
    kind: UploadKind = .ebook
) -> Bool {
    UploadFlow.canCommit(
        kind: kind, title: title, author: author, series: series, seriesIndex: index
    )
}

private func url(_ name: String, in folder: String = "picked") -> URL {
    URL(fileURLWithPath: "/tmp/\(folder)/\(name)")
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
        let accepted = UploadFlow.ebookExtensions.union(UploadFlow.audiobookExtensions)
        #expect(accepted.isSubset(of: extensions), "every accepted format must be pickable")
        // Exclusivity, not just membership — the previous assertion passed with
        // `mp4`/`mpg4` on the list because it only checked that four names were
        // present. `mpga` is the one documented residual: `public.mp3` claims it
        // as a sibling extension and no narrower UTType exists.
        #expect(extensions.subtracting(accepted) == ["mpga"])
    }

    @Test func acceptedAudioSetIsExactlyWhatTheUploadEndpointTakes() {
        // Pinned against the server's own list, not against the constant this
        // is defined from — asserting `audiobookExtensions == the set it is
        // assigned from` cannot fail, and the guards it replaced (no flac, no
        // wav) were the only thing catching a widening.
        #expect(UploadFlow.audiobookExtensions == ["m4b", "m4a", "mp3"])
        #expect(UploadFlow.ebookExtensions == ["epub"])
        for playableButNotUploadable in ["flac", "wav", "ogg", "opus", "aac"] {
            #expect(!UploadFlow.audiobookExtensions.contains(playableButNotUploadable))
        }
    }

    // MARK: - Length caps

    @Test func canCommitRejectsValuesTheServerWouldRefuseAfterFilingTheBook() {
        // The server validates overrides only after copying the file into the
        // library and reindexing, so an over-long title lands a book on disk,
        // answers 400, and duplicates it on the retry.
        let longTitle = String(repeating: "a", count: UploadFlow.titleMaxLength + 1)
        let longName = String(repeating: "a", count: UploadFlow.nameMaxLength + 1)
        #expect(!commits(title: longTitle, author: "Herbert"))
        #expect(!commits(title: "Dune", author: longName))
        #expect(
            commits(
                title: String(repeating: "a", count: UploadFlow.titleMaxLength),
                author: String(repeating: "a", count: UploadFlow.nameMaxLength)
            )
        )
    }

    @Test func capsCountScalarsTheWayTheServerDoes() {
        // `MetadataOverrides::validate` counts `chars()` — Unicode scalars —
        // while Swift's `String.count` counts grapheme clusters, which is never
        // larger. Measuring the wrong unit made the gate strictly looser than
        // the server's, so this exact title slipped through and 400'd *after*
        // the book was filed.
        let decomposed = String(repeating: "e\u{0301}", count: 300)
        #expect(decomposed.count == 300)
        #expect(UploadFlow.serverLength(of: decomposed) == 600)
        #expect(!commits(title: decomposed, author: "Herbert"))

        let author = String(repeating: "e\u{0301}", count: 130)
        #expect(author.count == 130)
        #expect(!commits(title: "Dune", author: author))
    }

    @Test func theMultipartByteCapIsUnreachableBehindTheScalarCap() {
        // Documents why no byte check is modelled: UTF-8 is at most 4 bytes per
        // scalar, so a title at the 500-scalar cap cannot exceed 2 KiB and the
        // server's 8 KiB per-field cap can never be the binding one.
        let widest = String(repeating: "\u{1F468}", count: UploadFlow.titleMaxLength)
        #expect(UploadFlow.serverLength(of: widest) == UploadFlow.titleMaxLength)
        #expect(widest.utf8.count == UploadFlow.titleMaxLength * 4)
        #expect(widest.utf8.count < 8 * 1024)
        #expect(commits(title: widest, author: "Herbert"))
    }

    @Test func optionalFieldsAreCappedOnlyWhereTheyAreSent() {
        let longName = String(repeating: "a", count: UploadFlow.nameMaxLength + 1)
        #expect(!commits(title: "Dune", author: "Herbert", series: longName))
        #expect(!commits(title: "Dune", author: "Herbert", index: longName))
        // The audiobook commit sends neither, so neither can block it.
        #expect(
            commits(
                title: "Dune", author: "Herbert", series: longName, index: longName,
                kind: .audiobook
            )
        )
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

    @Test func selectionGroupsMp3sFromOneFolderIntoOneMultiPartAudiobook() {
        // `classify_audio_set` files a set of .mp3 parts as a single book; one
        // request per file made an N-part audiobook into N one-chapter books.
        let picked = [url("01.mp3"), url("02.mp3"), url("03.mp3")]
        let selection = UploadFlow.selection(for: picked)
        #expect(selection.batches.count == 1)
        #expect(selection.batches.first?.kind == .audiobook)
        #expect(selection.batches.first?.urls == picked)
    }

    @Test func selectionKeepsMp3sFromDifferentFoldersAsSeparateBooks() {
        // Three standalone single-track audiobooks are the ordinary case that
        // one-batch-for-every-mp3 merged into a single book, recoverable only
        // by deleting it. The folder is the one signal available for which
        // parts actually belong together.
        let picked = [
            url("book-a.mp3", in: "A"), url("book-b.mp3", in: "B"), url("book-c.mp3", in: "C"),
        ]
        let selection = UploadFlow.selection(for: picked)
        #expect(selection.batches.count == 3)
        #expect(selection.batches.allSatisfy { $0.urls.count == 1 })
    }

    @Test func selectionGroupsPerFolderAcrossAMixedMp3Pick() {
        let picked = [
            url("01.mp3", in: "A"), url("01.mp3", in: "B"), url("02.mp3", in: "A"),
        ]
        let selection = UploadFlow.selection(for: picked)
        #expect(selection.batches.count == 2)
        // Folder A keeps both of its parts, in pick order, and folder A comes
        // first because its first part was picked first.
        #expect(selection.batches[0].urls == [url("01.mp3", in: "A"), url("02.mp3", in: "A")])
        #expect(selection.batches[1].urls == [url("01.mp3", in: "B")])
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

    @Test func displayNameNamesAMultiPartSetForItsFolder() {
        #expect(UploadBatch(kind: .ebook, urls: [url("Dune.epub")]).displayName == "Dune.epub")
        // Two picked sets rendered as "2 parts" apiece were indistinguishable
        // in both the row list and the confirm sheet's title.
        let dune = UploadBatch(
            kind: .audiobook, urls: [url("01.mp3", in: "Dune"), url("02.mp3", in: "Dune")]
        )
        #expect(dune.displayName == "Dune (2 parts)")
        let other = UploadBatch(
            kind: .audiobook, urls: [url("01.mp3", in: "Emma"), url("02.mp3", in: "Emma")]
        )
        #expect(other.displayName != dune.displayName)
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
        #expect(commits(title: "Dune", author: "Herbert"))
        #expect(!commits(title: "", author: "Herbert"))
        #expect(!commits(title: "Dune", author: "   "))
    }

    // MARK: - Endpoints

    @Test func kindsPointAtTheirOwnInspectAndCommitRoutes() {
        #expect(UploadKind.ebook.inspectPath == "/api/uploads/ebooks/inspect")
        #expect(UploadKind.ebook.commitPath == "/api/uploads/ebooks")
        #expect(UploadKind.audiobook.inspectPath == "/api/uploads/audiobooks/inspect")
        #expect(UploadKind.audiobook.commitPath == "/api/uploads/audiobooks")
    }
}
