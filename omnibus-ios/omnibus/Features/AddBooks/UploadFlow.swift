//  UploadFlow.swift
//  Pure request-shaping for the add-books upload: which ingest a picked file
//  targets, and how a selection groups into commits. Split from the view so
//  the rules the server enforces are testable without one, the same way
//  `CheckInFlow` carries the check-in stage machine.

import Foundation
import UniformTypeIdentifiers

/// Which ingest an upload targets. The two endpoints accept disjoint format
/// sets, so the file extension decides and there is no ambiguous case.
enum UploadKind: Equatable, Sendable {
    case ebook
    case audiobook

    var inspectPath: String {
        switch self {
        case .ebook: "/api/uploads/ebooks/inspect"
        case .audiobook: "/api/uploads/audiobooks/inspect"
        }
    }

    var commitPath: String {
        switch self {
        case .ebook: "/api/uploads/ebooks"
        case .audiobook: "/api/uploads/audiobooks"
        }
    }

    /// Series is an ebook-only field on the commit form — the audiobook
    /// endpoint reads `title`/`author` and nothing else.
    var acceptsSeries: Bool { self == .ebook }
}

/// One commit's worth of picked files: a single EPUB, a single `.m4a`/`.m4b`,
/// or every `.mp3` of one multi-part audiobook.
struct UploadBatch: Equatable, Sendable {
    var kind: UploadKind
    var urls: [URL]

    /// What the progress row and the confirm sheet call this upload.
    var displayName: String {
        guard let first = urls.first else { return "" }
        return urls.count == 1
            ? first.lastPathComponent
            : "\(urls.count) parts"
    }
}

/// The grouping of a picked selection: what will be uploaded, and what was
/// dropped because the server would not accept it.
struct UploadSelection: Equatable, Sendable {
    var batches: [UploadBatch] = []
    /// Filenames whose extension neither endpoint accepts, so the sheet can
    /// name them instead of failing them one 415 at a time.
    var unsupported: [String] = []
}

enum UploadFlow {
    /// Extensions `/api/uploads/ebooks` accepts. EPUB only — the magic-byte
    /// gate in `shared::detect_ebook_format` recognizes nothing else.
    static let ebookExtensions: Set<String> = ["epub"]

    /// Extensions `/api/uploads/audiobooks` accepts, matching `audiobook_ext_of`
    /// on the server. Deliberately narrower than `Book.audioFormats`, which
    /// describes what the library can *play* — offering `.flac` here would
    /// transfer a whole audiobook before the server answered 415.
    static let audiobookExtensions: Set<String> = ["m4a", "m4b", "mp3"]

    /// Which ingest a filename targets, or `nil` when neither accepts it.
    static func kind(for filename: String) -> UploadKind? {
        let ext = (filename as NSString).pathExtension.lowercased()
        if ebookExtensions.contains(ext) { return .ebook }
        if audiobookExtensions.contains(ext) { return .audiobook }
        return nil
    }

    /// `Content-Type` for a part. The server routes on the filename extension,
    /// so this is courtesy rather than contract — but sending `audio/mp4` for
    /// an MP3, as the sheet used to, is a lie worth not telling.
    static func mimeType(for filename: String) -> String {
        switch (filename as NSString).pathExtension.lowercased() {
        case "epub": "application/epub+zip"
        case "mp3": "audio/mpeg"
        case "m4a", "m4b": "audio/mp4"
        default: "application/octet-stream"
        }
    }

    /// Split a picked selection into one batch per book the server would file.
    ///
    /// Every `.mp3` in the selection becomes one multi-part audiobook, which is
    /// the only grouping `classify_audio_set` accepts; each EPUB and each
    /// `.m4a`/`.m4b` container is its own book, so they get a batch apiece
    /// rather than the 400 that sending two containers in one request earns.
    ///
    /// Batches follow the pick order, except that the MP3 set lands last: it
    /// isn't complete until the whole selection has been walked, so it cannot
    /// take the position of the first part it collected.
    static func selection(for urls: [URL]) -> UploadSelection {
        var result = UploadSelection()
        var mp3s: [URL] = []
        for url in urls {
            let name = url.lastPathComponent
            switch kind(for: name) {
            case .ebook:
                result.batches.append(UploadBatch(kind: .ebook, urls: [url]))
            case .audiobook where (name as NSString).pathExtension.lowercased() == "mp3":
                mp3s.append(url)
            case .audiobook:
                result.batches.append(UploadBatch(kind: .audiobook, urls: [url]))
            case nil:
                result.unsupported.append(name)
            }
        }
        if !mp3s.isEmpty {
            result.batches.append(UploadBatch(kind: .audiobook, urls: mp3s))
        }
        return result
    }

    /// The commit form fields for a confirmed upload, in a fixed order so the
    /// request body is deterministic. Blank optional fields are omitted rather
    /// than sent empty — `norm` on the server would discard them anyway, and
    /// an absent part is the honest way to say "no value".
    static func commitFields(
        kind: UploadKind, title: String, author: String, series: String, seriesIndex: String
    ) -> [(name: String, value: String)] {
        var fields: [(name: String, value: String)] = [
            ("title", title.trimmingCharacters(in: .whitespacesAndNewlines)),
            ("author", author.trimmingCharacters(in: .whitespacesAndNewlines)),
        ]
        guard kind.acceptsSeries else { return fields }
        let trimmedSeries = series.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedSeries.isEmpty { fields.append(("series", trimmedSeries)) }
        let trimmedIndex = seriesIndex.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedIndex.isEmpty { fields.append(("series_index", trimmedIndex)) }
        return fields
    }

    /// Whether the confirm sheet may submit. Both fields are required by the
    /// commit handlers, so the button is disabled rather than letting the
    /// server answer 400.
    static func canCommit(title: String, author: String) -> Bool {
        !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !author.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Content types the file importer offers — exactly what the two endpoints
    /// accept, so an upload cannot fail on format after the whole file has
    /// been transferred.
    static var pickerTypes: [UTType] {
        var types: [UTType] = [.epub, .mp3, .mpeg4Audio]
        for ext in ["m4b", "m4a"] {
            if let type = UTType(filenameExtension: ext), !types.contains(type) {
                types.append(type)
            }
        }
        return types
    }
}
