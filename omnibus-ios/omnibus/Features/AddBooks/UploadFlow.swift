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
/// or the `.mp3` parts of one multi-part audiobook.
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
    /// on the server. This is the same narrow set the player and downloader
    /// already use, so it is shared rather than restated — `Book.audioFormats`
    /// is the *broad* list of what a library may contain, and routing on that
    /// transferred whole `.flac`/`.wav` files the server then answered 415.
    static let audiobookExtensions: Set<String> = Book.selectableAudioFormats

    /// Longest title the server will accept, mirroring
    /// `MetadataOverrides::TITLE_MAX_LEN` in `shared/src/ebook/overrides.rs`.
    /// Enforced here because the server validates *after* filing the book, so
    /// an over-long title lands a book on disk and then answers 400.
    static let titleMaxLength = 500

    /// Longest author/series value, mirroring `MetadataOverrides::NAME_MAX_LEN`.
    static let nameMaxLength = 250

    /// Which ingest a filename targets, or `nil` when neither accepts it.
    static func kind(for filename: String) -> UploadKind? {
        let ext = fileExtension(of: filename)
        if ebookExtensions.contains(ext) { return .ebook }
        if audiobookExtensions.contains(ext) { return .audiobook }
        return nil
    }

    /// `Content-Type` for a part. The server routes on the filename extension,
    /// so this is courtesy rather than contract — but sending `audio/mp4` for
    /// an MP3, as the sheet used to, is a lie worth not telling.
    static func mimeType(for filename: String) -> String {
        switch fileExtension(of: filename) {
        case "epub": "application/epub+zip"
        case "mp3": "audio/mpeg"
        case "m4a", "m4b": "audio/mp4"
        default: "application/octet-stream"
        }
    }

    /// Split a picked selection into one batch per book the server would file.
    ///
    /// Each EPUB and each `.m4a`/`.m4b` container is its own book, so they get a
    /// batch apiece rather than the 400 that sending two containers in one
    /// request earns. `.mp3` files are grouped **by their containing folder**:
    /// `classify_audio_set` files a set of MP3s as one book, and a folder is the
    /// only signal available for which of them actually belong together. Lumping
    /// every picked MP3 into one book instead merged unrelated single-track
    /// audiobooks with no way back short of deleting the result.
    ///
    /// Batches follow the pick order, except that MP3 sets land last: a set
    /// isn't complete until the whole selection has been walked, so it cannot
    /// take the position of the first part it collected.
    static func selection(for urls: [URL]) -> UploadSelection {
        var result = UploadSelection()
        var mp3Folders: [URL] = []
        var mp3ByFolder: [URL: [URL]] = [:]

        for url in urls {
            let name = url.lastPathComponent
            switch kind(for: name) {
            case .ebook:
                result.batches.append(UploadBatch(kind: .ebook, urls: [url]))
            case .audiobook where fileExtension(of: name) == "mp3":
                let folder = url.deletingLastPathComponent()
                if mp3ByFolder[folder] == nil { mp3Folders.append(folder) }
                mp3ByFolder[folder, default: []].append(url)
            case .audiobook:
                result.batches.append(UploadBatch(kind: .audiobook, urls: [url]))
            case nil:
                result.unsupported.append(name)
            }
        }

        for folder in mp3Folders {
            guard let parts = mp3ByFolder[folder] else { continue }
            result.batches.append(UploadBatch(kind: .audiobook, urls: parts))
        }
        return result
    }

    /// The commit form fields for a confirmed upload, in a fixed order so the
    /// request body is deterministic.
    ///
    /// Blank optional fields are omitted. Note this means a *cleared* series
    /// cannot be pushed: the server reads an absent part as "keep the file's
    /// embedded value", and sending an empty string is identical because its
    /// `norm` trims-and-drops. Clearing needs a server-side absent-vs-empty
    /// distinction; until then the metadata editor is where a wrong series
    /// gets removed.
    static func commitFields(
        kind: UploadKind, title: String, author: String, series: String, seriesIndex: String
    ) -> [(name: String, value: String)] {
        var fields: [(name: String, value: String)] = [
            ("title", title.trimmingCharacters(in: .whitespacesAndNewlines)),
            ("author", author.trimmingCharacters(in: .whitespacesAndNewlines)),
        ]
        guard kind.acceptsSeries else { return fields }
        if let series = series.nilIfBlank { fields.append(("series", series)) }
        if let seriesIndex = seriesIndex.nilIfBlank {
            fields.append(("series_index", seriesIndex))
        }
        return fields
    }

    /// Whether the confirm sheet may submit.
    ///
    /// Both fields are required by the commit handlers, and both carry a length
    /// cap the server enforces only *after* the book is filed and indexed — so
    /// an over-long title would land a book on disk, answer 400, and file a
    /// second copy on the retry. The button stands down instead.
    static func canCommit(title: String, author: String) -> Bool {
        guard let title = title.nilIfBlank, let author = author.nilIfBlank else { return false }
        return title.count <= titleMaxLength && author.count <= nameMaxLength
    }

    /// Whether an optional confirm-sheet field is short enough to commit.
    static func isWithinNameCap(_ value: String) -> Bool {
        (value.nilIfBlank?.count ?? 0) <= nameMaxLength
    }

    /// Content types the file importer offers, derived from the accepted
    /// extensions so the two cannot drift.
    ///
    /// A `UTType` can carry sibling extensions the server does not take —
    /// `public.mp3` also claims `mpga` — so this narrows the picker rather than
    /// matching it exactly, and [`selection(for:)`] stays the real gate. It is
    /// still worth deriving: naming `UTType.mpeg4Audio` by hand offered `mp4`
    /// and `mpg4`, which are not audiobooks at all.
    static var pickerTypes: [UTType] {
        let extensions = ebookExtensions.sorted() + audiobookExtensions.sorted()
        var types: [UTType] = []
        for ext in extensions {
            guard let type = UTType(filenameExtension: ext), !types.contains(type) else { continue }
            types.append(type)
        }
        return types
    }

    /// Lowercased extension of a filename, the one place that parse happens.
    private static func fileExtension(of filename: String) -> String {
        (filename as NSString).pathExtension.lowercased()
    }
}
