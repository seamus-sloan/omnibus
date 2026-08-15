//  Models.swift
//  Codable mirrors of the `omnibus-shared` wire types. Field names and
//  optionality track `shared/src/*` exactly — the server is the contract.

import Foundation

// MARK: - Lenient decoding

/// The server omits `false` bools and empty collections from its JSON —
/// `#[serde(skip_serializing_if = "std::ops::Not::not")]` on `has_physical`,
/// `skip_serializing_if = "Vec::is_empty"` on `formats` / `book_files`, and so
/// on. Swift's synthesized `Decodable` ignores a property's default value and
/// hard-fails on a missing key, so these overloads supply the same defaults
/// serde would. Overload resolution prefers them over the generic protocol
/// requirement, so every synthesized initializer in this file picks them up.
///
/// Deliberately limited to `Bool` and arrays: those are the two shapes the
/// server actually elides. Defaulting `String` or `Int` would paper over a
/// genuine contract break instead of surfacing it.
extension KeyedDecodingContainer {
    func decode(_ type: Bool.Type, forKey key: Key) throws -> Bool {
        try decodeIfPresent(Bool.self, forKey: key) ?? false
    }

    func decode<T: Decodable>(_ type: [T].Type, forKey key: Key) throws -> [T] {
        try decodeIfPresent([T].self, forKey: key) ?? []
    }
}

// MARK: - Books

struct Contributor: Codable, Hashable, Sendable {
    var name: String
    var role: String?
    var fileAs: String?
    var id: Int64?

    enum CodingKeys: String, CodingKey {
        case name, role, id
        case fileAs = "file_as"
    }
}

struct Identifier: Codable, Hashable, Sendable {
    var value: String
    var scheme: String?
}

struct BookFileInfo: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var format: String
    var filename: String
    var ordinal: Int64
    var label: String?
    var sizeBytes: Int64 = 0
    var path: String?
    /// Content validator for this file, derived server-side from the same
    /// filesystem stat the reindex diff keys on. A download snapshots it and
    /// compares against a later metadata refresh to learn its copy is stale.
    /// `nil` for rows the scanner has not stat'd yet.
    var etag: String?

    enum CodingKeys: String, CodingKey {
        case id, format, filename, ordinal, label, path, etag
        case sizeBytes = "size_bytes"
    }
}

struct Book: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var filename: String
    var title: String?
    var description: String?
    var publisher: String?
    var published: String?
    var modified: String?
    var language: String?
    var creators: [Contributor] = []
    var subjects: [String] = []
    /// User-assigned genres. Distinct from `subjects`, which come from the
    /// EPUB's `<dc:subject>` entries — nothing the server parses carries a
    /// genre, so this list is populated only by an explicit edit. Omitted
    /// from the wire when empty, hence the default.
    var genres: [String] = []
    var identifiers: [Identifier] = []
    var isbn13: String?
    var series: String?
    var seriesIndex: String?
    var seriesId: Int64?
    var uniqueIdentifier: String?
    var coverURL: String?
    var accent: String?
    var formats: [String] = []
    var hasPhysical: Bool = false
    var addedAt: String?
    var error: String?
    var hasOverride: Bool = false
    var hasCoverOverride: Bool = false
    var bookFiles: [BookFileInfo] = []
    var epubSizeBytes: Int64?
    /// Page count of the book's CBZ archive, attached by the detail read for
    /// comic books only — the pager's slider range and progress mapping.
    /// `nil` on every other payload (list projections, non-comics).
    var pageCount: Int64?

    enum CodingKeys: String, CodingKey {
        case id, filename, title, description, publisher, published, modified
        case language, creators, subjects, genres, identifiers, isbn13, series, formats
        case accent, error
        case seriesIndex = "series_index"
        case seriesId = "series_id"
        case uniqueIdentifier = "unique_identifier"
        case coverURL = "cover_url"
        case hasPhysical = "has_physical"
        case addedAt = "added_at"
        case hasOverride = "has_override"
        case hasCoverOverride = "has_cover_override"
        case bookFiles = "book_files"
        case epubSizeBytes = "epub_size_bytes"
        case pageCount = "page_count"
    }

    /// Stable identity used for every `/books/:uuid` and `/api/covers/:uuid`
    /// URL. Falls back to the row id when a book predates the uuid backfill.
    var uuid: String { uniqueIdentifier ?? String(id) }

    var displayTitle: String {
        title?.nilIfBlank ?? filename
    }

    var authorDisplay: String {
        let names = creators.map(\.name).filter { !$0.isEmpty }
        return names.isEmpty ? "Unknown author" : names.joined(separator: ", ")
    }

    var year: String? {
        guard let published, published.count >= 4 else { return nil }
        return String(published.prefix(4))
    }

    var hasEbook: Bool {
        formats.contains { Self.ebookFormats.contains($0.lowercased()) }
    }

    var hasAudiobook: Bool {
        formats.contains { Self.audioFormats.contains($0.lowercased()) }
    }

    var hasEpub: Bool {
        formats.contains { $0.caseInsensitiveCompare("epub") == .orderedSame }
    }

    var hasComic: Bool {
        formats.contains { $0.caseInsensitiveCompare("cbz") == .orderedSame }
    }

    /// Whether "Read" opens the native comic pager rather than the EPUB
    /// reader. A book carrying both keeps the EPUB as its primary read —
    /// the same rule the web pager and the server's `/file` resolution use.
    var opensAsComic: Bool { hasComic && !hasEpub }

    static let ebookFormats: Set<String> = ["epub", "kepub", "pdf", "mobi", "azw3", "cbz", "cbr"]
    static let audioFormats: Set<String> = ["m4b", "m4a", "mp3", "aac", "flac", "ogg", "opus", "wav"]

    /// The formats the server's manifest resolver admits for playback —
    /// `resolve_audiobook_file` in `db/src/hls/query.rs` filters on exactly
    /// these, so offering any other format as a selection would 404.
    static let selectableAudioFormats: Set<String> = ["m4b", "m4a", "mp3"]

    /// The audiobook files a listener can choose between, in the order the
    /// server resolves them — its default (no `file_id`) is the first of
    /// these by ordinal.
    var audioFiles: [BookFileInfo] {
        bookFiles
            .filter { Self.selectableAudioFormats.contains($0.format.lowercased()) }
            .sorted { $0.ordinal < $1.ordinal }
    }
}

struct EbookLibrary: Codable, Sendable {
    var path: String?
    var books: [Book] = []
    var error: String?
    var total: Int64?
}

// MARK: - Discovery

struct AuthorSummary: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var name: String
    var sort: String?
    var bookCount: Int
    var accent: String?
    var hasPhoto: Bool = false

    enum CodingKeys: String, CodingKey {
        case id, name, sort, accent
        case bookCount = "book_count"
        case hasPhoto = "has_photo"
    }
}

struct AuthorDetail: Codable, Sendable {
    var id: Int64
    var name: String
    var sort: String?
    var bookCount: Int
    var books: [Book] = []
    var hasPhoto: Bool = false

    enum CodingKeys: String, CodingKey {
        case id, name, sort, books
        case bookCount = "book_count"
        case hasPhoto = "has_photo"
    }
}

struct SeriesSummary: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var name: String
    var sort: String?
    var bookCount: Int
    var primaryAuthor: String?
    var accent: String?

    enum CodingKeys: String, CodingKey {
        case id, name, sort, accent
        case bookCount = "book_count"
        case primaryAuthor = "primary_author"
    }
}

struct SeriesDetail: Codable, Sendable {
    var id: Int64
    var name: String
    var sort: String?
    var bookCount: Int
    var books: [Book] = []

    enum CodingKeys: String, CodingKey {
        case id, name, sort, books
        case bookCount = "book_count"
    }
}

struct TagWeight: Codable, Hashable, Sendable, Identifiable {
    var name: String
    var count: Int
    var id: String { name }
}

/// Structurally a `TagWeight`, kept a distinct type because `/api/genres` and
/// `/api/tags` are two different vocabularies — mirroring the server's
/// `GenreWeight` / `TagWeight` split.
struct GenreWeight: Codable, Hashable, Sendable, Identifiable {
    var name: String
    var count: Int
    var id: String { name }
}

// MARK: - Search palette

struct PaletteBookHit: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var uuid: String = ""
    var title: String
    var authorDisplay: String
    var year: String?
    var formats: [String] = []
    var coverURL: String?
    var accent: String?

    enum CodingKeys: String, CodingKey {
        case id, uuid, title, year, formats, accent
        case authorDisplay = "author_display"
        case coverURL = "cover_url"
    }
}

struct PaletteAuthorHit: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var name: String
    var bookCount: UInt32
    var leadBookTitle: String?

    enum CodingKeys: String, CodingKey {
        case id, name
        case bookCount = "book_count"
        case leadBookTitle = "lead_book_title"
    }
}

struct PaletteSeriesHit: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var name: String
    var bookCount: UInt32
    var authorDisplay: String?
    var leadBookTitle: String?

    enum CodingKeys: String, CodingKey {
        case id, name
        case bookCount = "book_count"
        case authorDisplay = "author_display"
        case leadBookTitle = "lead_book_title"
    }
}

struct PaletteTagHit: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var name: String
    var bookCount: UInt32

    enum CodingKeys: String, CodingKey {
        case id, name
        case bookCount = "book_count"
    }
}

struct PaletteResults: Codable, Sendable {
    var query: String = ""
    var books: [PaletteBookHit] = []
    var authors: [PaletteAuthorHit] = []
    var series: [PaletteSeriesHit] = []
    var tags: [PaletteTagHit] = []
    var durationMs: UInt64 = 0
    var bookTotal: UInt32 = 0
    var authorTotal: UInt32 = 0
    var seriesTotal: UInt32 = 0
    var tagTotal: UInt32 = 0

    enum CodingKeys: String, CodingKey {
        case query, books, authors, series, tags
        case durationMs = "duration_ms"
        case bookTotal = "book_total"
        case authorTotal = "author_total"
        case seriesTotal = "series_total"
        case tagTotal = "tag_total"
    }

    var isEmpty: Bool {
        books.isEmpty && authors.isEmpty && series.isEmpty && tags.isEmpty
    }

    /// The books-only answer the local mirror can produce.
    ///
    /// Authors, series, and tags are entities the server indexes and the
    /// device doesn't, so a local answer carries books alone and the server's
    /// fuller one replaces it a moment later. `bookTotal` is the count shown,
    /// not a claim about the whole library — there is no "see all N" to offer
    /// until the server has weighed in.
    init(localBooks: [Book]) {
        self.books = localBooks.map { book in
            PaletteBookHit(
                id: book.id,
                uuid: book.uuid,
                title: book.displayTitle,
                authorDisplay: book.authorDisplay,
                year: book.year,
                formats: book.formats,
                coverURL: book.coverURL,
                accent: book.accent
            )
        }
        self.bookTotal = UInt32(localBooks.count)
    }

    init() {}
}

// MARK: - Auth

struct UserSummary: Codable, Hashable, Sendable {
    var id: Int64
    var username: String
    var isAdmin: Bool
    var canUpload: Bool
    var canEdit: Bool
    var canDownload: Bool
    var kindleEmail: String?
    /// Presentation name other users see. `nil` falls back to `username`,
    /// which stays the login identity.
    var displayName: String?
    /// Whether this user has uploaded an avatar. Non-optional: the lenient
    /// `decode(Bool.Type,forKey:)` above defaults a missing key to `false`, so
    /// a pre-upgrade `CacheKey.me` blob still decodes with the right meaning.
    var hasAvatar: Bool = false
    /// Formats this user hides from the library's All Books view, canonical
    /// lowercase tokens ("cbz"). The lenient `decode([T].Type,forKey:)` above
    /// defaults a missing key to `[]`, so a pre-upgrade `CacheKey.me` blob
    /// still decodes.
    var hiddenFormats: [String] = []

    /// The name to show for this user — never render `username` on its own.
    var display: String { displayName ?? username }

    enum CodingKeys: String, CodingKey {
        case id, username
        case isAdmin = "is_admin"
        case canUpload = "can_upload"
        case canEdit = "can_edit"
        case canDownload = "can_download"
        case kindleEmail = "kindle_email"
        case displayName = "display_name"
        case hasAvatar = "has_avatar"
        case hiddenFormats = "hidden_formats"
    }
}

struct LoginRequest: Encodable, Sendable {
    var username: String
    var password: String
    var clientKind: String? = "ios"
    var deviceName: String?
    var clientVersion: String?

    enum CodingKeys: String, CodingKey {
        case username, password
        case clientKind = "client_kind"
        case deviceName = "device_name"
        case clientVersion = "client_version"
    }
}

struct LoginResponse: Decodable, Sendable {
    var user: UserSummary
    var token: String?
}

// MARK: - Progress

enum ProgressFormat: String, Codable, Sendable {
    case epub
    case audio
}

struct ProgressUpdate: Codable, Sendable {
    var bookUUID: String
    var format: ProgressFormat
    var epubCFI: String?
    var audioPositionSeconds: Double?
    /// Whole-book percent, 0...100 — the cross-surface half of a position.
    /// Set by the comic pager (whose `epubCFI` is a page anchor, not a CFI)
    /// so the landing surfaces can draw a bar without parsing the anchor.
    var progressPercent: Int64?
    /// When the reader actually moved here, by this device's clock.
    ///
    /// Stamped at the gesture rather than left to the server, because a write
    /// made offline can reach it hours later — long after another device has
    /// read further. The server keeps whichever position carries the later
    /// clock, so a replayed position can no longer overwrite a newer one.
    var clientUpdatedAt: Int64 = Int64(Date().timeIntervalSince1970)
    /// The `book_files` row the position was taken in, when the client knows
    /// it. `nil` encodes as an absent field, so a server that predates the
    /// column sees the same payload it always has.
    var bookFileID: Int64?

    enum CodingKeys: String, CodingKey {
        case format
        case bookUUID = "book_uuid"
        case epubCFI = "epub_cfi"
        case audioPositionSeconds = "audio_position_seconds"
        case progressPercent = "progress_percent"
        case clientUpdatedAt = "client_updated_at"
        case bookFileID = "book_file_id"
    }
}

struct ProgressRecord: Codable, Sendable {
    var bookUUID: String
    var format: ProgressFormat
    var epubCFI: String?
    var audioPositionSeconds: Double?
    /// Whole-book percent, 0...100, when the write carried one — a comic
    /// position always does, a Kobo's percent-only write does, a CFI-only
    /// EPUB save does not.
    var progressPercent: Int64?
    /// When the server stored this row, on the *server's* clock.
    var updatedAt: Int64
    /// When the reader moved here, on the clock of the device that moved them.
    ///
    /// `nil` only against a server too old to send it. Prefer
    /// [`orderingClock`] over reading either field directly.
    var clientUpdatedAt: Int64?
    /// The `book_files` row the position was taken in. `nil` for positions
    /// saved before the column existed, and against older servers.
    var bookFileID: Int64?

    /// The clock two positions may be compared on.
    ///
    /// `updatedAt` cannot be: an optimistic row this device wrote carries a
    /// device clock and a row that came back from the server carries the
    /// server's arrival clock, so comparing them measured the drift between
    /// two boxes' idea of the time rather than which position was further
    /// along. A phone running a few minutes ahead of a self-hosted server
    /// suppressed every sync offer; one running behind raised them constantly.
    /// The server orders conflicts on the reader's clock, so that is the one
    /// number both sides can agree about.
    var orderingClock: Int64 { clientUpdatedAt ?? updatedAt }

    enum CodingKeys: String, CodingKey {
        case format
        case bookUUID = "book_uuid"
        case epubCFI = "epub_cfi"
        case audioPositionSeconds = "audio_position_seconds"
        case progressPercent = "progress_percent"
        case updatedAt = "updated_at"
        case clientUpdatedAt = "client_updated_at"
        case bookFileID = "book_file_id"
    }
}

extension ProgressUpdate {
    /// This write as the record it asserts, for comparing against what the
    /// replica already holds. The device clock fills both timestamp fields —
    /// this is the only clock the write carries until the server answers.
    var asRecord: ProgressRecord {
        ProgressRecord(
            bookUUID: bookUUID,
            format: format,
            epubCFI: epubCFI,
            audioPositionSeconds: audioPositionSeconds,
            progressPercent: progressPercent,
            updatedAt: clientUpdatedAt,
            clientUpdatedAt: clientUpdatedAt,
            bookFileID: bookFileID
        )
    }
}

struct ResumePoint: Codable, Sendable, Identifiable {
    var record: ProgressRecord
    var book: Book
    var totalDurationSeconds: Double?
    var chapterNumber: Int64?
    var chapterCount: Int64?
    /// The saved playback rate for this book's audio, so the hero's "left"
    /// readout can show the wall-clock wait. `nil` for epub rows, when no
    /// preference is saved (1x), and against older servers.
    var playbackRate: Double?

    /// Scoped by format as well as book, mirroring `CacheKey.progress`.
    ///
    /// A dual-format book someone is both reading and listening to produces two
    /// of these, and keying them on the uuid alone gave them one identity: the
    /// `ForEach`es that render the Continue rail saw a duplicate id, shuffled
    /// per-card state between the two, and left the carousel's page count
    /// disagreeing with its dots.
    var id: String { "\(record.bookUUID):\(record.format.rawValue)" }

    var isAudio: Bool { record.format == .audio }

    /// Whether this is the card `record` belongs to — the (book, format) pair
    /// behind [`id`], single-sourced so no caller re-derives it as the uuid.
    func matches(_ record: ProgressRecord) -> Bool {
        self.record.bookUUID == record.bookUUID && self.record.format == record.format
    }

    enum CodingKeys: String, CodingKey {
        case record, book
        case totalDurationSeconds = "total_duration_seconds"
        case chapterNumber = "chapter_number"
        case chapterCount = "chapter_count"
        case playbackRate = "playback_rate"
    }

    /// Fraction complete for the progress bar, when the format supports one.
    ///
    /// Audio derives it from the position over the whole-book duration. A
    /// reading record has one only when it carries the cross-surface percent
    /// — a comic position always does, a CFI-only EPUB save does not, and
    /// there is no honest percentage to derive from a bare CFI. The format
    /// check is what keeps a reading card from borrowing the listening
    /// card's bar.
    var fraction: Double? {
        if isAudio {
            guard let total = totalDurationSeconds, total > 0,
                  let position = record.audioPositionSeconds else { return nil }
            return min(1, max(0, position / total))
        }
        guard let percent = record.progressPercent else { return nil }
        return min(1, max(0, Double(percent) / 100))
    }
}

struct SessionReport: Codable, Sendable {
    var bookUUID: String
    var format: ProgressFormat
    var startedAt: Int64
    var endedAt: Int64
    var progressUnits: Int64
    var deviceId: Int64?
    /// Minted here so a replay is idempotent. A report whose reply was lost
    /// stays queued and is retried; without a handle the server appended a
    /// second row each time and the reading time it represents was counted
    /// twice. Resolved server-side against a unique index (migration 0052).
    var clientID: String = AnnotationID.mint()

    enum CodingKeys: String, CodingKey {
        case format
        case bookUUID = "book_uuid"
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case progressUnits = "progress_units"
        case deviceId = "device_id"
        case clientID = "client_id"
    }
}

// MARK: - Audiobooks

struct ManifestPart: Codable, Hashable, Sendable, Identifiable {
    var ordinal: Int64
    var url: String
    var durationSeconds: Double
    var mime: String

    var id: Int64 { ordinal }

    enum CodingKeys: String, CodingKey {
        case ordinal, url, mime
        case durationSeconds = "duration_seconds"
    }
}

struct ChapterInfo: Codable, Hashable, Sendable, Identifiable {
    var ordinal: Int64
    var title: String
    var startSeconds: Double
    var durationSeconds: Double

    var id: Int64 { ordinal }

    enum CodingKeys: String, CodingKey {
        case ordinal, title
        case startSeconds = "start_seconds"
        case durationSeconds = "duration_seconds"
    }
}

/// `#[serde(tag = "mode", rename_all = "lowercase")]` on the Rust side.
enum AudiobookManifest: Codable, Sendable {
    case direct(parts: [ManifestPart], totalDuration: Double, chapters: [ChapterInfo])
    case hls(playlistURL: String)

    enum CodingKeys: String, CodingKey {
        case mode, parts, chapters
        case totalDuration = "total_duration_seconds"
        case playlistURL = "playlist_url"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .mode) {
        case "direct":
            self = .direct(
                parts: try c.decode([ManifestPart].self, forKey: .parts),
                totalDuration: try c.decode(Double.self, forKey: .totalDuration),
                chapters: try c.decodeIfPresent([ChapterInfo].self, forKey: .chapters) ?? []
            )
        case "hls":
            self = .hls(playlistURL: try c.decode(String.self, forKey: .playlistURL))
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .mode, in: c, debugDescription: "unknown manifest mode \(other)"
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .direct(parts, totalDuration, chapters):
            try c.encode("direct", forKey: .mode)
            try c.encode(parts, forKey: .parts)
            try c.encode(totalDuration, forKey: .totalDuration)
            try c.encode(chapters, forKey: .chapters)
        case let .hls(playlistURL):
            try c.encode("hls", forKey: .mode)
            try c.encode(playlistURL, forKey: .playlistURL)
        }
    }

    var chapters: [ChapterInfo] {
        if case let .direct(_, _, chapters) = self { return chapters }
        return []
    }

    var totalDuration: Double? {
        if case let .direct(_, total, _) = self { return total }
        return nil
    }
}

struct AudiobookPlaybackRateUpdate: Codable, Sendable {
    var playbackRate: Double

    enum CodingKeys: String, CodingKey {
        case playbackRate = "playback_rate"
    }
}

struct AudiobookPlaybackRateRecord: Codable, Sendable {
    var bookUUID: String
    var playbackRate: Double
    var updatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case bookUUID = "book_uuid"
        case playbackRate = "playback_rate"
        case updatedAt = "updated_at"
    }
}

// MARK: - Annotations

enum HighlightColor: String, Codable, CaseIterable, Sendable {
    case amber, green, blue, rose, violet
}

struct Highlight: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var bookUUID: String
    /// `nil` for Kobo-origin annotations — anchored by an opaque KoboSpan on
    /// the server, so they list here but cannot be painted in the reader.
    var epubCFIRange: String?
    var color: HighlightColor
    var note: String?
    var text: String?
    /// The handle this device minted when the highlight was made. Present on
    /// anything this app created; `nil` for rows the web client wrote.
    var clientID: String?
    var createdAt: Int64

    /// What to put in a request path addressing this highlight. The minted
    /// handle wins when there is one: an annotation created offline has no
    /// server id yet, and `AnnotationID.pending` is not a row anywhere.
    var pathID: String { clientID ?? String(id) }

    enum CodingKeys: String, CodingKey {
        case id, color, note, text
        case bookUUID = "book_uuid"
        case epubCFIRange = "epub_cfi_range"
        case clientID = "client_id"
        case createdAt = "created_at"
    }
}

struct CreateHighlight: Codable, Sendable {
    var bookUUID: String
    var epubCFIRange: String
    var color: HighlightColor
    var text: String?
    var clientID: String

    enum CodingKeys: String, CodingKey {
        case color, text
        case bookUUID = "book_uuid"
        case epubCFIRange = "epub_cfi_range"
        case clientID = "client_id"
    }
}

struct Bookmark: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var bookUUID: String
    var position: String
    var title: String?
    var clientID: String?
    var createdAt: Int64

    var pathID: String { clientID ?? String(id) }

    enum CodingKeys: String, CodingKey {
        case id, position, title
        case bookUUID = "book_uuid"
        case clientID = "client_id"
        case createdAt = "created_at"
    }
}

struct CreateBookmark: Codable, Sendable {
    var bookUUID: String
    var position: String
    var title: String?
    var clientID: String

    enum CodingKeys: String, CodingKey {
        case position, title
        case bookUUID = "book_uuid"
        case clientID = "client_id"
    }
}

/// Identity for an annotation the device has created but the server may not
/// have seen yet.
///
/// Every highlight and bookmark this app makes is minted here at the moment of
/// the gesture, and that handle — not the server's row id — is what every
/// later op names. It is what lets a highlight created and then deleted while
/// offline replay coherently: both ops address the same thing, and the server
/// resolves the handle to whatever row the create ended up producing.
enum AnnotationID {
    static func mint() -> String { UUID().uuidString }

    /// Whether this row exists only on this device — it has been created but
    /// the outbox hasn't landed it yet, so the server has no id for it.
    static func isPending(_ id: Int64) -> Bool { id < 0 }

    /// Stand-in row id for an annotation that exists only on this device.
    /// Negative so it can never collide with a server id, and distinct per
    /// annotation so SwiftUI's `ForEach` keeps them apart.
    static func pending() -> Int64 { -Int64(abs(UUID().hashValue % 1_000_000_000)) - 1 }
}

extension CreateHighlight {
    /// Mint the handle as part of building the payload, so no call site can
    /// forget to and leave an annotation unaddressable while offline.
    init(bookUUID: String, epubCFIRange: String, color: HighlightColor, text: String?) {
        self.init(
            bookUUID: bookUUID, epubCFIRange: epubCFIRange, color: color,
            text: text, clientID: AnnotationID.mint()
        )
    }

    /// The row to show immediately, before the server has seen this at all.
    var optimistic: Highlight {
        Highlight(
            id: AnnotationID.pending(), bookUUID: bookUUID, epubCFIRange: epubCFIRange,
            color: color, note: nil, text: text, clientID: clientID,
            createdAt: Int64(Date().timeIntervalSince1970)
        )
    }
}

extension CreateBookmark {
    init(bookUUID: String, position: String, title: String?) {
        self.init(
            bookUUID: bookUUID, position: position, title: title,
            clientID: AnnotationID.mint()
        )
    }

    var optimistic: Bookmark {
        Bookmark(
            id: AnnotationID.pending(), bookUUID: bookUUID, position: position,
            title: title, clientID: clientID,
            createdAt: Int64(Date().timeIntervalSince1970)
        )
    }
}

// MARK: - Ratings & read status

struct RatingUpdate: Codable, Sendable {
    var bookUUID: String
    var stars: Double

    enum CodingKeys: String, CodingKey {
        case stars
        case bookUUID = "book_uuid"
    }
}

struct RatingRecord: Codable, Sendable {
    var bookUUID: String
    var stars: Double
    var updatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case stars
        case bookUUID = "book_uuid"
        case updatedAt = "updated_at"
    }
}

struct AttributedRating: Codable, Sendable, Identifiable {
    var userId: Int64
    var username: String
    /// Whether the rater has an avatar. Non-optional: the lenient
    /// `decode(Bool.Type,forKey:)` above defaults a missing key to `false`,
    /// so a pre-upgrade `CacheKey.ratingsOthers` blob still decodes.
    var hasAvatar: Bool = false
    var stars: Double
    var updatedAt: Int64

    var id: Int64 { userId }

    enum CodingKeys: String, CodingKey {
        case username, stars
        case userId = "user_id"
        case hasAvatar = "has_avatar"
        case updatedAt = "updated_at"
    }
}

enum ReadStatus: String, Codable, CaseIterable, Sendable {
    case unread, reading, finished

    var label: String {
        switch self {
        case .unread: "Unread"
        case .reading: "Reading"
        case .finished: "Finished"
        }
    }
}

struct SetReadStatus: Codable, Sendable {
    var bookUUID: String
    var status: ReadStatus

    enum CodingKeys: String, CodingKey {
        case status
        case bookUUID = "book_uuid"
    }
}

struct ReadStatusRecord: Codable, Sendable {
    var bookUUID: String
    var status: ReadStatus
    var updatedAt: Int64
    var finishedAt: Int64?

    enum CodingKeys: String, CodingKey {
        case status
        case bookUUID = "book_uuid"
        case updatedAt = "updated_at"
        case finishedAt = "finished_at"
    }
}

// MARK: - Journals

enum JournalStatus: String, Codable, Sendable {
    case draft, published
}

struct JournalEntry: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var bookUUID: String
    var authorId: Int64
    var authorName: String
    /// Whether the author has an avatar. Non-optional: the lenient
    /// `decode(Bool.Type,forKey:)` above defaults a missing key to `false`,
    /// so a pre-upgrade `CacheKey.journals` blob still decodes.
    var authorHasAvatar: Bool = false
    var bodyMd: String
    var bodyHtml: String
    var progress: Int?
    var status: JournalStatus = .published
    var clientID: String?
    var createdAt: Int64
    var updatedAt: Int64

    var pathID: String { clientID ?? String(id) }

    enum CodingKeys: String, CodingKey {
        case id, progress, status
        case bookUUID = "book_uuid"
        case authorId = "author_id"
        case authorName = "author_name"
        case authorHasAvatar = "author_has_avatar"
        case bodyMd = "body_md"
        case bodyHtml = "body_html"
        case clientID = "client_id"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

struct CreateJournalEntry: Codable, Sendable {
    var bookUUID: String
    var bodyMd: String
    var progress: Int?
    var status: JournalStatus = .published
    var clientID: String = AnnotationID.mint()

    enum CodingKeys: String, CodingKey {
        case progress, status
        case bookUUID = "book_uuid"
        case bodyMd = "body_md"
        case clientID = "client_id"
    }
}

/// Body for `PATCH /api/journals/{id}`. Editing is what makes a draft
/// finishable — without it, saving a draft is a one-way trip.
struct UpdateJournalEntry: Codable, Sendable {
    var bodyMd: String
    var progress: Int?
    var status: JournalStatus?

    enum CodingKeys: String, CodingKey {
        case progress, status
        case bodyMd = "body_md"
    }
}

// MARK: - Shelves

enum ShelfKind: String, Codable, Sendable {
    case smart, manual, wishlist

    var isSystem: Bool { self == .wishlist }
}

enum ShelfVisibility: String, Codable, Sendable {
    case `private`, `public`
}

enum MatchMode: String, Codable, Sendable {
    case any, all
}

enum RuleField: String, Codable, CaseIterable, Sendable {
    case tag, genre, author, series, rating, status, format, year
    case dateAdded = "date_added"
    case dateUpdated = "date_updated"

    var label: String {
        switch self {
        case .tag: "Tag"
        case .genre: "Genre"
        case .author: "Author"
        case .series: "Series"
        case .rating: "Rating"
        case .status: "Status"
        case .format: "Format"
        case .year: "Year"
        case .dateAdded: "Date added"
        case .dateUpdated: "Date updated"
        }
    }

    /// Mirrors `RuleField::accepts` so the rule editor only offers operators
    /// the server will accept.
    var acceptedOps: [RuleOp] {
        switch self {
        case .tag, .genre, .author, .series: [.is, .isNot, .contains, .startsWith]
        case .rating: [.is, .gte]
        case .status: [.is]
        case .format: [.includes, .contains, .startsWith]
        case .year: [.is, .gte]
        case .dateAdded, .dateUpdated: [.inLast, .between, .before, .after]
        }
    }
}

enum RuleOp: String, Codable, CaseIterable, Sendable {
    case `is`
    case isNot = "is_not"
    case contains
    case startsWith = "starts_with"
    case gte
    case includes
    case inLast = "in_last"
    case between, before, after

    var label: String {
        switch self {
        case .is: "is"
        case .isNot: "is not"
        case .contains: "contains"
        case .startsWith: "starts with"
        case .gte: "is at least"
        case .includes: "includes"
        case .inLast: "in the last"
        case .between: "between"
        case .before: "before"
        case .after: "after"
        }
    }
}

struct ShelfRule: Codable, Hashable, Sendable {
    var field: RuleField
    var op: RuleOp
    var value: String
}

/// Compact row for the shelves index (`GET /api/shelves`).
struct ShelfSummary: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var ownerUserId: Int64
    var ownerUsername: String
    var kind: ShelfKind
    var name: String
    var visibility: ShelfVisibility
    var accent: String?
    var bookCount: Int64

    enum CodingKeys: String, CodingKey {
        case id, kind, name, visibility, accent
        case ownerUserId = "owner_user_id"
        case ownerUsername = "owner_username"
        case bookCount = "book_count"
    }
}

/// Full detail (`GET /api/shelves/{id}`) — summary fields plus rules.
struct Shelf: Codable, Hashable, Sendable, Identifiable {
    var id: Int64
    var ownerUserId: Int64
    var ownerUsername: String
    var kind: ShelfKind
    var name: String
    var description: String?
    var visibility: ShelfVisibility
    var accent: String?
    var matchMode: MatchMode?
    var rules: [ShelfRule] = []
    var bookCount: Int64

    enum CodingKeys: String, CodingKey {
        case id, kind, name, description, visibility, accent, rules
        case ownerUserId = "owner_user_id"
        case ownerUsername = "owner_username"
        case matchMode = "match_mode"
        case bookCount = "book_count"
    }
}

/// A shelf plus the first few covers on it, for the card treatment.
struct ShelfPreview: Codable, Sendable, Identifiable {
    var shelf: ShelfSummary
    var covers: [Book]

    var id: Int64 { shelf.id }
}

struct ShelfPage: Codable, Sendable {
    var books: [Book] = []
}

struct CreateShelfRequest: Codable, Sendable {
    var kind: ShelfKind
    var name: String
    var description: String?
    var visibility: ShelfVisibility = .private
    var matchMode: MatchMode?
    var rules: [ShelfRule] = []
    var bookUUIDs: [String] = []

    enum CodingKeys: String, CodingKey {
        case kind, name, description, visibility, rules
        case matchMode = "match_mode"
        case bookUUIDs = "book_uuids"
    }
}

struct RulePreview: Codable, Sendable {
    var matched: Int64
    var total: Int64
    var sample: [Book] = []
}

struct RulePreviewRequest: Codable, Sendable {
    var matchMode: MatchMode
    var rules: [ShelfRule]

    enum CodingKeys: String, CodingKey {
        case rules
        case matchMode = "match_mode"
    }
}

// MARK: - Stats

enum StatsRange: String, Codable, CaseIterable, Sendable {
    case week, month, year
    case allTime = "all_time"

    var label: String {
        switch self {
        case .week: "Week"
        case .month: "Month"
        case .year: "Year"
        case .allTime: "Lifetime"
        }
    }
}

struct DayActivity: Codable, Hashable, Sendable, Identifiable {
    var day: String
    var seconds: Int64
    var id: String { day }
}

struct RankedEntity: Codable, Hashable, Sendable, Identifiable {
    var name: String
    var seconds: Int64
    var id: String { name }
}

struct GenreShare: Codable, Hashable, Sendable, Identifiable {
    var name: String
    var books: Int64
    var id: String { name }
}

struct FinishedBook: Codable, Hashable, Sendable, Identifiable {
    var bookUUID: String
    var title: String
    var author: String?
    var finishedAt: Int64
    var coverURL: String?
    var rating: Double?

    var id: String { bookUUID + String(finishedAt) }

    enum CodingKeys: String, CodingKey {
        case title, author, rating
        case bookUUID = "book_uuid"
        case finishedAt = "finished_at"
        case coverURL = "cover_url"
    }
}

struct MonthCount: Codable, Hashable, Sendable, Identifiable {
    var month: String
    var books: Int64
    var id: String { month }
}

struct StatsSummary: Codable, Sendable {
    var range: StatsRange = .month
    var readingSeconds: Int64 = 0
    var listeningSeconds: Int64 = 0
    var avgStars: Double?
    var sessions: Int64 = 0
    var activeDays: Int64 = 0
    var longestStreakDays: Int64 = 0
    var busiestWeekStart: String?
    var busiestWeekSeconds: Int64 = 0
    var booksFinished: Int64 = 0
    var booksActive: Int64 = 0
    var asOfDay: String = ""
    var heatmap: [DayActivity] = []
    var topAuthors: [RankedEntity] = []
    var topTags: [RankedEntity] = []
    var genreShare: [GenreShare] = []
    var finishedBooks: [FinishedBook] = []
    var booksPerMonth: [MonthCount] = []
    var pagesRead: Int64?

    enum CodingKeys: String, CodingKey {
        case range, sessions, heatmap
        case readingSeconds = "reading_seconds"
        case listeningSeconds = "listening_seconds"
        case avgStars = "avg_stars"
        case activeDays = "active_days"
        case longestStreakDays = "longest_streak_days"
        case busiestWeekStart = "busiest_week_start"
        case busiestWeekSeconds = "busiest_week_seconds"
        case booksFinished = "books_finished"
        case booksActive = "books_active"
        case asOfDay = "as_of_day"
        case topAuthors = "top_authors"
        case topTags = "top_tags"
        case genreShare = "genre_share"
        case finishedBooks = "finished_books"
        case booksPerMonth = "books_per_month"
        case pagesRead = "pages_read"
    }

    var totalSeconds: Int64 { readingSeconds + listeningSeconds }
}

// MARK: - Settings & health

struct LibrarySettings: Codable, Sendable {
    var ebookLibraryPath: String?
    var audiobookLibraryPath: String?

    enum CodingKeys: String, CodingKey {
        case ebookLibraryPath = "ebook_library_path"
        case audiobookLibraryPath = "audiobook_library_path"
    }
}

struct HealthResponse: Codable, Sendable {
    var status: String?
    var version: String?
}

// MARK: - Check-in / scan

struct ScanResolveRequest: Codable, Sendable {
    var isbn: String
}

struct ScanBook: Codable, Hashable, Sendable {
    var uuid: String
    var title: String
    var authors: [String]
    var coverURL: String?
    var hasPhysical: Bool
    var isbn: String?

    enum CodingKeys: String, CodingKey {
        case uuid, title, authors, isbn
        case coverURL = "cover_url"
        case hasPhysical = "has_physical"
    }
}

struct ExternalBookMeta: Codable, Hashable, Sendable {
    var isbn13: String
    var title: String
    var authors: [String]
    var year: String?
    var pages: Int64?
    var publisher: String?
    var description: String?
    var coverURL: String?
    /// Series statement, best-effort (Open Library enrichment). Defaulted so
    /// pre-existing call sites and older servers' responses stay valid.
    var series: String? = nil
    /// Year the work was first published across all editions; `year` is this
    /// edition's own date.
    var firstPublishYear: Int64? = nil
    var source: String

    enum CodingKeys: String, CodingKey {
        case isbn13, title, authors, year, pages, publisher, description, series, source
        case coverURL = "cover_url"
        case firstPublishYear = "first_publish_year"
    }

    var authorDisplay: String {
        authors.isEmpty ? "Unknown author" : authors.joined(separator: ", ")
    }
}

/// `POST /api/scan/search` — title-text search, the fallback when an ISBN
/// resolves to `unresolved`.
struct ScanSearchRequest: Codable, Sendable {
    var query: String
}

/// The search answer: provider candidates, each complete enough to feed
/// `ResolveMetaRequest`.
struct ScanSearchResponse: Codable, Sendable {
    var results: [ExternalBookMeta]
}

/// `POST /api/scan/resolve-meta` — resolve a picked search candidate against
/// the library without a second provider round trip.
struct ResolveMetaRequest: Codable, Sendable {
    var meta: ExternalBookMeta
}

/// `#[serde(tag = "kind", rename_all = "snake_case")]` on the Rust side.
enum ScanOutcome: Decodable, Equatable, Sendable {
    case alreadyOwned(book: ScanBook)
    case onWishlist(book: ScanBook)
    case inLibraryUnowned(book: ScanBook)
    /// Every library row whose (title, author) matched, in server order — an
    /// EPUB and the audiobook nothing attached to it are one work in two rows,
    /// so this is a picker, never empty.
    case closeMatch(books: [ScanBook], scanned: ExternalBookMeta)
    case notInLibrary(online: ExternalBookMeta)
    case unresolved

    enum CodingKeys: String, CodingKey {
        case kind, book, others, scanned, online
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "already_owned":
            self = .alreadyOwned(book: try c.decode(ScanBook.self, forKey: .book))
        case "on_wishlist":
            self = .onWishlist(book: try c.decode(ScanBook.self, forKey: .book))
        case "in_library_unowned":
            self = .inLibraryUnowned(book: try c.decode(ScanBook.self, forKey: .book))
        case "close_match":
            // Head + tail on the wire, so a build that predates the picker
            // still decodes `book` alone; `others` is absent for the common
            // single-candidate match.
            let first = try c.decode(ScanBook.self, forKey: .book)
            let others = try c.decodeIfPresent([ScanBook].self, forKey: .others) ?? []
            self = .closeMatch(
                books: [first] + others,
                scanned: try c.decode(ExternalBookMeta.self, forKey: .scanned)
            )
        case "not_in_library":
            self = .notInLibrary(online: try c.decode(ExternalBookMeta.self, forKey: .online))
        default:
            self = .unresolved
        }
    }
}

struct CheckInRequest: Codable, Sendable {
    var bookUUID: String
    var isbn: String?
    var note: String?

    enum CodingKeys: String, CodingKey {
        case isbn, note
        case bookUUID = "book_uuid"
    }
}

struct AddPhysicalOnlyRequest: Codable, Sendable {
    var meta: ExternalBookMeta
    var note: String?
}

/// `#[serde(rename_all = "lowercase")]` on the Rust side.
enum WishlistSource: String, Codable, Sendable {
    case scan, detail, manual
}

/// Names its target with either an existing `book_uuid` or resolved `meta`;
/// `book_uuid` wins when both are set. `source` is required — the server has no
/// default for it, so omitting it fails the body deserialization outright. A
/// wishlist entry carries no note (a physical copy's check-in does).
struct WishlistAddRequest: Codable, Sendable {
    var bookUUID: String?
    var meta: ExternalBookMeta?
    var source: WishlistSource

    enum CodingKeys: String, CodingKey {
        case meta, source
        case bookUUID = "book_uuid"
    }
}

/// `GET /api/physical/{uuid}/wishlist` — the caller's tracking entry for a
/// book, or a JSON `null` (decoded as `nil`) when the book isn't wishlisted.
struct WishlistEntry: Codable, Equatable, Sendable {
    var id: Int64
    var userID: Int64
    var bookUUID: String
    var addedAt: Int64
    var source: WishlistSource

    enum CodingKeys: String, CodingKey {
        case id, source
        case userID = "user_id"
        case bookUUID = "book_uuid"
        case addedAt = "added_at"
    }
}

/// `Json(BookRef { book_uuid })` — returned by all three scan writes.
struct BookRef: Codable, Sendable {
    var bookUUID: String

    enum CodingKeys: String, CodingKey {
        case bookUUID = "book_uuid"
    }
}

// MARK: - Small helpers

extension String {
    var nilIfBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
