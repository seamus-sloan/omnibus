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
    /// Secondary ISBN-10. Unlike `isbn13` — which the server derives from the
    /// scanned `identifiers` when no override exists — no format Omnibus reads
    /// carries a distinct ISBN-10, so this is only ever an explicit edit.
    var isbn10: String?
    var series: String?
    var seriesIndex: String?
    var seriesId: Int64?
    var uniqueIdentifier: String?
    var coverURL: String?
    var accent: String?
    var formats: [String] = []
    var hasPhysical: Bool = false
    var addedAt: String?
    /// Most recent moment *anyone* touched this book — the axis behind the
    /// "Recently interacted" sort. Library-wide, not per-reader: the server
    /// folds it at read time from ratings, published journal entries, read
    /// status, check-ins, and the book's own `last_modified`, none of them
    /// scoped to the caller. So it can move for a book this reader has never
    /// opened, and it moves without the book's own metadata changing.
    var lastInteractedAt: String?
    var error: String?
    var hasOverride: Bool = false
    var hasCoverOverride: Bool = false
    var bookFiles: [BookFileInfo] = []
    var epubSizeBytes: Int64?
    /// Page count of the book's CBZ archive, attached by the detail read for
    /// comic books only — the pager's slider range and progress mapping.
    /// `nil` on every other payload (list projections, non-comics).
    var pageCount: Int64?
    /// Print edition page count. Deliberately *not* `pageCount`, which is the
    /// CBZ archive's image count and drives the comic pager's slider — the two
    /// mean different things on a book that has both.
    var printPages: Int64?

    enum CodingKeys: String, CodingKey {
        case id, filename, title, description, publisher, published, modified
        case language, creators, subjects, genres, identifiers, isbn13, isbn10, series, formats
        case accent, error
        case seriesIndex = "series_index"
        case seriesId = "series_id"
        case uniqueIdentifier = "unique_identifier"
        case coverURL = "cover_url"
        case hasPhysical = "has_physical"
        case addedAt = "added_at"
        case lastInteractedAt = "last_interacted_at"
        case hasOverride = "has_override"
        case hasCoverOverride = "has_cover_override"
        case bookFiles = "book_files"
        case epubSizeBytes = "epub_size_bytes"
        case pageCount = "page_count"
        case printPages = "print_pages"
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
    /// Whether this reader's book detail page uses the snap-stop marquee.
    /// The lenient `decode(Bool.Type,forKey:)` above defaults a missing key to
    /// `false`, so a pre-0092 `/me` payload (or a cached blob from one) still
    /// decodes, with the off default this setting ships as.
    var bookDetailScrollStops: Bool = false

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
        case bookDetailScrollStops = "book_detail_scroll_stops"
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

/// How far a ``ResolvedPosition``'s chapter attribution can be trusted.
/// Reported rather than withheld: a coarse answer the caller knows is coarse
/// beats an absent one it fills in by guessing.
enum PositionConfidence: String, Codable, Sendable {
    case high
    case low
}

/// Where a stored position sits in the book, resolved server-side against the
/// spine and table of contents (reading) or the container's marks (audio).
///
/// Every field is optional because the structure behind it may be missing — a
/// TOC-less EPUB, an audiobook with no chapter marks — and ``confidence`` says
/// how much of the block to lean on.
struct ResolvedPosition: Codable, Sendable, Equatable {
    var spineIndex: Int64?
    var chapterTitle: String?
    /// 1-based, so `chapterOrdinal` of ``chaptersTotal`` reads directly.
    var chapterOrdinal: Int64?
    var chaptersTotal: Int64?
    var percentThroughChapter: Int64?
    var percentThroughBook: Int64?
    var confidence: PositionConfidence

    enum CodingKeys: String, CodingKey {
        case spineIndex = "spine_index"
        case chapterTitle = "chapter_title"
        case chapterOrdinal = "chapter_ordinal"
        case chaptersTotal = "chapters_total"
        case percentThroughChapter = "percent_through_chapter"
        case percentThroughBook = "percent_through_book"
        case confidence
    }
}

/// Every position the reader holds in one book — the body of
/// `GET /api/progress/{uuid}`.
///
/// Returned whole rather than one format at a time: a reader 87% through the
/// audiobook and 47% through the EPUB has one true place, and ``furthest``
/// names it. `?format=` narrows ``records`` when a caller genuinely wants one
/// side, which is what the per-format reconcile does.
struct BookProgress: Codable, Sendable {
    var bookUUID: String
    var records: [ProgressRecord]
    var furthest: ProgressFormat?
    var linked: Bool = false

    enum CodingKeys: String, CodingKey {
        case bookUUID = "book_uuid"
        case records, furthest, linked
    }

    /// The record for one format, or `nil` when the reader has no position in it.
    func record(for format: ProgressFormat) -> ProgressRecord? {
        records.first { $0.format == format }
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
    /// Whole-book audio duration, so a listening position becomes a percent
    /// without this device sourcing a runtime from anywhere else. `nil` for
    /// reading rows, and on the echo a write returns — the server fills it
    /// on read paths only.
    var totalDurationSeconds: Double?
    /// Where this position sits in the book, resolved server-side. `nil` on
    /// the same terms as ``totalDurationSeconds``.
    var resolved: ResolvedPosition?

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
        case totalDurationSeconds = "total_duration_seconds"
        case resolved
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
    /// 1-based structural part of the audiobook timeline, from the resolved
    /// file's marks. **Not a book chapter**: a 65-chapter novel stored as a
    /// 4-part M4B carries four marks, and calling that "chapter 4 of 4" read
    /// as the end of the book. Real chapters live in ``ProgressRecord/resolved``.
    var audioPart: Int64?
    var audioPartCount: Int64?
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
        case audioPart = "audio_part"
        case audioPartCount = "audio_part_count"
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
            guard let total = record.totalDurationSeconds, total > 0,
                  let position = record.audioPositionSeconds else { return nil }
            return min(1, max(0, position / total))
        }
        guard let percent = record.progressPercent else { return nil }
        return min(1, max(0, Double(percent) / 100))
    }

    /// Which structural position to name beside this card.
    ///
    /// A confidently resolved chapter wins — it is what the reader means by
    /// "where am I". A low-confidence block is not demoted to nothing but to
    /// the part readout, which is what it actually measured.
    var structuralPosition: StructuralPosition? {
        if let resolved = record.resolved, resolved.confidence == .high,
           let ordinal = resolved.chapterOrdinal {
            return .chapter(ordinal: ordinal, total: resolved.chaptersTotal)
        }
        guard let part = audioPart else { return nil }
        return .part(ordinal: part, total: audioPartCount)
    }
}

/// The structural position a resume surface names — a real chapter, or the
/// coarser audiobook part when that is all the container supports.
enum StructuralPosition: Equatable, Sendable {
    case chapter(ordinal: Int64, total: Int64?)
    case part(ordinal: Int64, total: Int64?)
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
    /// Minutes east of UTC on this device when the session was captured —
    /// `-420` in Los Angeles, `330` in Kolkata. The server buckets the
    /// time-of-day and day-of-week charts against this rather than against
    /// UTC, so a reader outside UTC sees their own evening as an evening and
    /// a session recorded while travelling stays anchored where it happened.
    ///
    /// Defaulted at init, which is capture time for every call site here:
    /// each of them builds the report as it ends the sitting.
    var utcOffsetMinutes: Int64? = SessionReport.localOffsetMinutes()
    /// This device's IANA zone name at capture time — `"America/Los_Angeles"`.
    ///
    /// Recorded **alongside** ``utcOffsetMinutes``, not instead of it. The
    /// offset says what the clock read, which is all the time-of-day charts
    /// need and is DST-correct for the instant it was taken; the zone says
    /// *where*, which an offset cannot — `-420` is Los Angeles in summer,
    /// Phoenix year-round and Denver in winter. Only a zone can resolve an
    /// offset for a **different** instant than the captured one.
    ///
    /// Nothing on the server reads it yet (resolving a zone wants a tz
    /// database). It is sent now because it cannot be recovered from an offset
    /// after the fact. Defaulted at init, like the offset beside it.
    var timeZone: String? = SessionReport.localTimeZoneName()

    /// This device's current offset from UTC, in whole minutes.
    ///
    /// `zone` is a seam for the tests. The sign is the bug this can really
    /// have — the web tracker has to negate JavaScript's opposite convention
    /// to reach the same number — and a test that re-derives it from
    /// `TimeZone.current` cannot catch a flip.
    static func localOffsetMinutes(in zone: TimeZone = .current) -> Int64 {
        Int64(zone.secondsFromGMT() / 60)
    }

    /// This device's IANA zone identifier. `zone` is a seam for the tests, as
    /// above.
    static func localTimeZoneName(in zone: TimeZone = .current) -> String? {
        let name = zone.identifier
        // A blank would encode as a present-but-empty value, which the
        // server's `SessionReport::validate` rejects outright — failing the
        // whole batched report over a field nothing reads yet.
        return name.isEmpty ? nil : name
    }

    enum CodingKeys: String, CodingKey {
        case format
        case bookUUID = "book_uuid"
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case progressUnits = "progress_units"
        case deviceId = "device_id"
        case clientID = "client_id"
        case utcOffsetMinutes = "utc_offset_minutes"
        case timeZone = "time_zone"
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

    /// The record for a book nobody has marked — what the server means by the
    /// `null` body it answers with when there is no row. Both clocks are zero
    /// because no write has ever happened.
    static func unmarked(uuid: String) -> ReadStatusRecord {
        ReadStatusRecord(bookUUID: uuid, status: .unread, updatedAt: 0, finishedAt: nil)
    }

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

/// One point of a labelled series — a day or a month, and the figure recorded
/// against it. Mirrors `omnibus_shared::TrendPoint`; the label's calendar is
/// the field's, not this type's.
struct TrendPoint: Codable, Hashable, Sendable, Identifiable {
    var label: String = ""
    var value: Double = 0

    var id: String { label }
}

/// One bar of the star-rating distribution: a half-star bucket and how many
/// books the reader rated into it within the window.
///
/// The wire scale is half-stars (1...10), so `starLabel` is the conversion
/// every surface must use — labelling `halfStars` raw would present the chart
/// as a ten-point scale.
struct RatingBucket: Codable, Hashable, Sendable, Identifiable {
    var halfStars: Int64
    var books: Int64

    enum CodingKeys: String, CodingKey {
        case books
        case halfStars = "half_stars"
    }

    var id: Int64 { halfStars }
    var stars: Double { Double(halfStars) / 2.0 }

    /// Axis label in stars — "0.5", "1", "1.5" … "5".
    var starLabel: String {
        halfStars % 2 == 0 ? "\(halfStars / 2)" : String(format: "%.1f", stars)
    }
}

/// One bar of the book-length distribution: a page-range label and how many
/// books finished in the window fall into it.
///
/// The server owns the boundaries *and* their labels, so nothing here
/// re-derives a range. One bucket is the unknown bucket — a book no rung of
/// the length ladder can measure — and it is rendered rather than dropped.
struct LengthBucket: Codable, Hashable, Sendable, Identifiable {
    var label: String
    var books: Int64
    var id: String { label }
}

/// A library-scale total and the coverage behind it.
///
/// The pair travels together on purpose: every input is nullable-or-zero in a
/// way that means *not measured yet*, and a bare total would report a
/// partly-backfilled library as a smaller one with total confidence.
struct MeasuredTotal: Codable, Hashable, Sendable {
    var total: Int64 = 0
    /// Books that contributed. `0` means nothing has been measured for this
    /// figure, which the Stats tab renders as an absent row, not a zero.
    var books: Int64 = 0

    var isEmpty: Bool { books == 0 }
}

/// How big the library is in words, pages, and hours of audio.
///
/// Library-scoped, not user-scoped, and fetched separately from
/// `StatsSummary` — it is the same answer for every reader and only moves on a
/// reindex, so carrying it on the per-user payload would re-send it on every
/// range change.
struct LibrarySize: Codable, Sendable {
    /// Live books — the denominator every coverage figure is read against.
    var books: Int64 = 0
    var words = MeasuredTotal()
    var pages = MeasuredTotal()
    var listeningSeconds = MeasuredTotal()

    enum CodingKeys: String, CodingKey {
        case books, words, pages
        case listeningSeconds = "listening_seconds"
    }

    init() {}

    /// Field-by-field for the same reason `StatsSummary` is: the Rust fields
    /// are `#[serde(default)]`, and an app ahead of its server must lose a
    /// figure rather than the whole screen.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        books = try c.decodeIfPresent(Int64.self, forKey: .books) ?? 0
        words = try c.decodeIfPresent(MeasuredTotal.self, forKey: .words) ?? MeasuredTotal()
        pages = try c.decodeIfPresent(MeasuredTotal.self, forKey: .pages) ?? MeasuredTotal()
        listeningSeconds =
            try c.decodeIfPresent(MeasuredTotal.self, forKey: .listeningSeconds) ?? MeasuredTotal()
    }

    var isEmpty: Bool { words.isEmpty && pages.isEmpty && listeningSeconds.isEmpty }
}

/// One column of the time-of-day strip: an hour of the reader's **local** day
/// and the active seconds recorded in it, reading and listening together.
///
/// The hour is resolved server-side from the UTC offset each session carried
/// at capture time — never from this device's zone, which would make the same
/// account read differently on a phone abroad than on the desktop at home.
/// All 24 arrive, ascending, zeros included.
struct HourBucket: Codable, Hashable, Sendable, Identifiable {
    var hour: Int64
    var seconds: Int64
    var id: Int64 { hour }

    /// `21:00` — a bare "21" beside a duration reads ambiguously.
    var clockLabel: String { String(format: "%02d:00", hour) }
}

/// One column of the day-of-week strip: a weekday in the reader's local
/// calendar and the active seconds recorded on it.
///
/// `weekday` is 0 = Monday ... 6 = Sunday, and `label` comes down with it:
/// week-start is a convention, and deciding it here would silently draw every
/// column one place out from what the web renders. All 7 arrive, Monday
/// first, zeros included.
struct WeekdayBucket: Codable, Hashable, Sendable, Identifiable {
    var weekday: Int64
    var label: String
    var seconds: Int64
    var id: Int64 { weekday }
}

/// One bucket of a library-composition dimension: a display label and the
/// distinct live books behind it.
struct CompositionSlice: Codable, Hashable, Sendable, Identifiable {
    var label: String = ""
    var books: Int64 = 0

    var id: String { label }
}

/// One dimension of the library's composition — its buckets plus the coverage
/// behind them.
///
/// The coverage pair is read one level up from `MeasuredTotal`'s usual sense:
/// `total` is **bucket placements** and `books` is the **distinct live books**
/// the dimension describes, so `total - books` is the overlap of books landing
/// in more than one bucket.
struct CompositionDimension: Codable, Hashable, Sendable {
    var slices: [CompositionSlice] = []
    var coverage = MeasuredTotal()

    init(slices: [CompositionSlice] = [], coverage: MeasuredTotal = MeasuredTotal()) {
        self.slices = slices
        self.coverage = coverage
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        slices = try c.decodeIfPresent([CompositionSlice].self, forKey: .slices) ?? []
        coverage = try c.decodeIfPresent(MeasuredTotal.self, forKey: .coverage) ?? MeasuredTotal()
    }

    /// True when no live book carries this dimension at all — the Stats tab's
    /// signal to render a sentence rather than an axis with no bars on it.
    var isEmpty: Bool { coverage.books == 0 || slices.isEmpty }

    /// Books that land in more than one bucket. Zero for every dimension whose
    /// buckets are mutually exclusive.
    var overlap: Int64 { max(0, coverage.total - coverage.books) }
}

/// What the collection is made of: its format, language, publisher,
/// publication-decade, and genre mix.
///
/// Library-scoped, not user-scoped, and fetched separately from
/// `StatsSummary` — the same answer for every reader, moving only on a
/// reindex, so carrying it on the per-user payload would re-send it on every
/// range change.
struct LibraryComposition: Codable, Sendable {
    /// Live books — those with at least one surviving file. The denominator
    /// every dimension's coverage is read against.
    var books: Int64 = 0
    /// Books indexed once whose files are gone. Reported rather than dropped:
    /// they carry no format at all, so they would otherwise vanish from the
    /// format mix and leave its counts failing to reconcile.
    var ghostedBooks: Int64 = 0
    var formats = CompositionDimension()
    var languages = CompositionDimension()
    var publishers = CompositionDimension()
    var decades = CompositionDimension()
    /// Genres have no link table by design, so this describes only the books
    /// someone has hand-edited. Read `genres.coverage` before its slices.
    var genres = CompositionDimension()

    enum CodingKeys: String, CodingKey {
        case books, formats, languages, publishers, decades, genres
        case ghostedBooks = "ghosted_books"
    }

    init() {}

    /// Field-by-field for the same reason `LibrarySize` is: the Rust fields
    /// are `#[serde(default)]`, and an app ahead of its server must lose a
    /// dimension rather than the whole screen.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        books = try c.decodeIfPresent(Int64.self, forKey: .books) ?? 0
        ghostedBooks = try c.decodeIfPresent(Int64.self, forKey: .ghostedBooks) ?? 0
        let dim = { (key: CodingKeys) throws -> CompositionDimension in
            try c.decodeIfPresent(CompositionDimension.self, forKey: key) ?? CompositionDimension()
        }
        formats = try dim(.formats)
        languages = try dim(.languages)
        publishers = try dim(.publishers)
        decades = try dim(.decades)
        genres = try dim(.genres)
    }

    /// True when the library has nothing to describe — the Stats tab's signal
    /// to render no section at all rather than five empty ones.
    var isEmpty: Bool {
        books == 0
            || (formats.isEmpty && languages.isEmpty && publishers.isEmpty && decades.isEmpty
                && genres.isEmpty)
    }
}

/// What the Pages read tile could and could not measure in the window, and the
/// day before which it cannot measure anything at all.
///
/// Mirrors `omnibus_shared::PagesReadDetail`. The tile's empty state is not one
/// state: a window with no activity, a window of listening only (audio has no
/// page analogue, so zero pages is the *correct* answer rather than an unknown
/// one), and real reading in books whose length nothing resolves are three
/// different facts. The server owns the distinction so this tile and the web
/// one cannot disagree about which of them a window is in.
struct PagesReadDetail: Codable, Sendable {
    var sinceDay: String?
    var measuredBooks: Int64 = 0
    var unmeasuredBooks: Int64 = 0
    var audioBooks: Int64 = 0
    /// Pages per UTC day inside the window, active days only, ascending.
    ///
    /// Decoded for one caller: the daily-goals card states today's pages even
    /// when no pages target is set, and `dailyGoals.pages` is target-gated —
    /// absent precisely when there is no goal. Today's entry is in every
    /// window (each of them ends today) and carries the same figure whichever
    /// one is showing, so reading a *standing* card off a windowed field is
    /// safe here in a way it would not be for a period total.
    var daily: [TrendPoint] = []
    /// Whether this window opens before `sinceDay`, so part of it is
    /// unmeasurable. Server-computed against the window's real start — the
    /// range alone does not answer it, and only the server knows where a
    /// period begins.
    var windowPredatesLedger = false

    enum CodingKeys: String, CodingKey {
        case daily
        case sinceDay = "since_day"
        case measuredBooks = "measured_books"
        case unmeasuredBooks = "unmeasured_books"
        case audioBooks = "audio_books"
        case windowPredatesLedger = "window_predates_ledger"
    }

    init() {}

    /// Field-by-field for the same reason `StatsSummary` is: the synthesized
    /// decoder ignores property defaults, so one missing key would throw.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sinceDay = try c.decodeIfPresent(String.self, forKey: .sinceDay)
        measuredBooks = try c.decodeIfPresent(Int64.self, forKey: .measuredBooks) ?? 0
        unmeasuredBooks = try c.decodeIfPresent(Int64.self, forKey: .unmeasuredBooks) ?? 0
        audioBooks = try c.decodeIfPresent(Int64.self, forKey: .audioBooks) ?? 0
        daily = try c.decodeIfPresent([TrendPoint].self, forKey: .daily) ?? []
        windowPredatesLedger =
            try c.decodeIfPresent(Bool.self, forKey: .windowPredatesLedger) ?? false
    }

    /// True when the window holds listening and no reading at all — the one
    /// empty state whose honest headline is `0`, not an em-dash.
    var audioOnly: Bool { audioBooks > 0 && measuredBooks == 0 && unmeasuredBooks == 0 }

    /// True when the window starts before the ledger did, so part of it is
    /// unmeasurable — and there is an epoch to name in the disclosure.
    var predatesLedger: Bool { sinceDay != nil && windowPredatesLedger }
}

/// One superlative that names a book: which book won, and the figure it won
/// with.
///
/// `value`'s unit is the *field's*, not this type's — pages for the length
/// superlatives, seconds for the longest sit, days for the fastest read. The
/// rows in `StatsView` supply it; nothing here should guess.
struct BookSuperlative: Codable, Hashable, Sendable {
    var bookUUID: String
    var title: String
    var author: String?
    var value: Int64

    enum CodingKeys: String, CodingKey {
        case title, author, value
        case bookUUID = "book_uuid"
    }
}

/// The window's single most-X figures. Every field is optional, and an absent
/// one means the window can't support that superlative — it is omitted, never
/// rendered as a zero or an em-dash.
struct Superlatives: Codable, Sendable {
    var longestBook: BookSuperlative?
    var shortestBook: BookSuperlative?
    var biggestDay: DayActivity?
    var longestSit: BookSuperlative?
    /// Fewest days from a book's first *tracked* session to its completion. A
    /// lower bound — reading done before session tracking, or on a device that
    /// reports nothing, is invisible here — which the section states.
    var fastestRead: BookSuperlative?

    enum CodingKeys: String, CodingKey {
        case longestBook = "longest_book"
        case shortestBook = "shortest_book"
        case biggestDay = "biggest_day"
        case longestSit = "longest_sit"
        case fastestRead = "fastest_read"
    }

    init() {}

    /// Decoded field-by-field for the same reason `StatsSummary` is: every
    /// field is `#[serde(default)]` on the Rust side, and an app running
    /// ahead of its server must lose a row rather than the whole tab.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        longestBook = try c.decodeIfPresent(BookSuperlative.self, forKey: .longestBook)
        shortestBook = try c.decodeIfPresent(BookSuperlative.self, forKey: .shortestBook)
        biggestDay = try c.decodeIfPresent(DayActivity.self, forKey: .biggestDay)
        longestSit = try c.decodeIfPresent(BookSuperlative.self, forKey: .longestSit)
        fastestRead = try c.decodeIfPresent(BookSuperlative.self, forKey: .fastestRead)
    }

    var isEmpty: Bool {
        longestBook == nil && shortestBook == nil && biggestDay == nil && longestSit == nil
            && fastestRead == nil
    }

    /// Recorded time a book needs before the server will crown it the fastest
    /// read — `omnibus_shared::FASTEST_READ_MIN_SECS`. Mirrored here so the
    /// caveat on the Stats tab states the real floor rather than a sentence
    /// that quietly stops being true when the server's changes.
    static let fastestReadMinSeconds: Int64 = 1800
}

struct StatsSummary: Codable, Sendable {
    var range: StatsRange = .month
    var readingSeconds: Int64 = 0
    var listeningSeconds: Int64 = 0
    var avgStars: Double?
    var sessions: Int64 = 0
    var activeDays: Int64 = 0
    var longestStreakDays: Int64 = 0
    /// The run of consecutive active days still going as of `asOfDay`, where
    /// `longestStreakDays` is the record. Server-computed rather than derived
    /// from `heatmap` here, so this tab and any widget can't disagree — and
    /// unwindowed on the server, so the tile reads the same whichever range
    /// the picker is on.
    var currentStreakDays: Int64 = 0
    var busiestWeekStart: String?
    var busiestWeekSeconds: Int64 = 0
    var booksFinished: Int64 = 0
    var booksActive: Int64 = 0
    var asOfDay: String = ""
    var heatmap: [DayActivity] = []
    var topAuthors: [RankedEntity] = []
    var topTags: [RankedEntity] = []
    var genreShare: [GenreShare] = []
    /// Distinct books with a genre *and* activity in the window — the
    /// population `genreShare`'s slices are drawn from, and the donut's centre
    /// count. Always `<= booksActive`; the difference is reading the ring
    /// cannot describe.
    var genreTaggedBooks: Int64 = 0
    var finishedBooks: [FinishedBook] = []
    var booksPerMonth: [MonthCount] = []
    /// The window's ratings by half-star bucket — the shape `avgStars`
    /// flattens away. All ten buckets arrive, zeros included.
    var ratingHistogram: [RatingBucket] = []
    /// Pages actually turned in the window — the ground each book was carried
    /// over, scaled by its resolved length. Not the length of the books
    /// finished in it. `nil` means nothing measurable; see `pagesDetail` before
    /// rendering that as "no data".
    var pagesRead: Int64?
    /// Estimated pages an hour over the window — the rate `pagesRead` is
    /// missing. Weighted by seconds across the books finished in the window
    /// that resolve a non-zero length and carry recorded reading time;
    /// listening time is excluded, so a partly-heard book reads fast here.
    /// `nil` when no finished book qualifies.
    var pagesPerHour: Double?
    /// Books finished in the window by length, plus the unknown bucket. Every
    /// bucket arrives; an all-zero set means nothing was finished.
    var lengthBuckets: [LengthBucket] = []
    /// Active seconds by local hour of day — all 24, ascending, zeros
    /// included, so the shape of a day stays readable. Reading and listening
    /// together, like every other activity metric here.
    var hourOfDay: [HourBucket] = []
    /// Active seconds by local weekday — all 7, Monday first, zeros included.
    /// Same sessions as `hourOfDay`, so the two strips always sum alike.
    var dayOfWeek: [WeekdayBucket] = []
    /// Active seconds in the window from sessions carrying no capture-time
    /// UTC offset, and so not placeable on a local clock — rows written
    /// before the server started recording one. Disclosed rather than folded
    /// in: bucketing them as UTC would put a reader's evening at 4am.
    var unzonedSeconds: Int64 = 0
    /// The window's single most-X figures. Every field inside is optional; an
    /// all-empty set means the window supports no superlative and the section
    /// is omitted.
    var superlatives = Superlatives()
    /// The reader's goal for the current calendar year, `nil` when none is
    /// set. Unwindowed, like `currentStreakDays` — a goal is annual, so it
    /// reads the same whichever range the picker is on.
    var goal: ReadingGoal?
    /// The reader's standing daily goals and today's progress toward them.
    /// Unwindowed for the same reason `goal` is: a daily target recurs, so it
    /// reads the same whichever range the picker is on.
    var dailyGoals = DailyGoals()
    /// The immediately preceding window's aggregates — the baseline the
    /// windowed tiles draw their deltas against.
    var previous = PeriodComparison()
    /// What `pagesRead` could and could not see, plus the day its ledger began.
    var pagesDetail = PagesReadDetail()

    enum CodingKeys: String, CodingKey {
        case range, sessions, heatmap
        case readingSeconds = "reading_seconds"
        case listeningSeconds = "listening_seconds"
        case avgStars = "avg_stars"
        case activeDays = "active_days"
        case longestStreakDays = "longest_streak_days"
        case currentStreakDays = "current_streak_days"
        case busiestWeekStart = "busiest_week_start"
        case busiestWeekSeconds = "busiest_week_seconds"
        case booksFinished = "books_finished"
        case booksActive = "books_active"
        case asOfDay = "as_of_day"
        case topAuthors = "top_authors"
        case topTags = "top_tags"
        case previous
        case genreShare = "genre_share"
        case genreTaggedBooks = "genre_tagged_books"
        case dailyGoals = "daily_goals"
        case finishedBooks = "finished_books"
        case booksPerMonth = "books_per_month"
        case ratingHistogram = "rating_histogram"
        case pagesRead = "pages_read"
        case pagesPerHour = "pages_per_hour"
        case lengthBuckets = "length_buckets"
        case hourOfDay = "hour_of_day"
        case dayOfWeek = "day_of_week"
        case unzonedSeconds = "unzoned_seconds"
        case superlatives
        case goal
        case pagesDetail = "pages_detail"
    }

    init() {}

    /// Decoded field-by-field with `decodeIfPresent`, because Swift's
    /// synthesized `init(from:)` **ignores property defaults** — a single
    /// missing key throws and `StatsView` renders its error state instead of
    /// the tab.
    ///
    /// Most of these are `#[serde(default)]` on the Rust side precisely so an
    /// older payload stays parseable, and that promise has to be kept on this
    /// side of the wire too: the app ships ahead of a self-hosted server
    /// routinely, and a stats field it hasn't learned yet must cost one tile,
    /// not the whole screen.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        range = try c.decodeIfPresent(StatsRange.self, forKey: .range) ?? .month
        readingSeconds = try c.decodeIfPresent(Int64.self, forKey: .readingSeconds) ?? 0
        listeningSeconds = try c.decodeIfPresent(Int64.self, forKey: .listeningSeconds) ?? 0
        avgStars = try c.decodeIfPresent(Double.self, forKey: .avgStars)
        sessions = try c.decodeIfPresent(Int64.self, forKey: .sessions) ?? 0
        activeDays = try c.decodeIfPresent(Int64.self, forKey: .activeDays) ?? 0
        longestStreakDays = try c.decodeIfPresent(Int64.self, forKey: .longestStreakDays) ?? 0
        currentStreakDays = try c.decodeIfPresent(Int64.self, forKey: .currentStreakDays) ?? 0
        busiestWeekStart = try c.decodeIfPresent(String.self, forKey: .busiestWeekStart)
        busiestWeekSeconds = try c.decodeIfPresent(Int64.self, forKey: .busiestWeekSeconds) ?? 0
        booksFinished = try c.decodeIfPresent(Int64.self, forKey: .booksFinished) ?? 0
        booksActive = try c.decodeIfPresent(Int64.self, forKey: .booksActive) ?? 0
        asOfDay = try c.decodeIfPresent(String.self, forKey: .asOfDay) ?? ""
        heatmap = try c.decodeIfPresent([DayActivity].self, forKey: .heatmap) ?? []
        topAuthors = try c.decodeIfPresent([RankedEntity].self, forKey: .topAuthors) ?? []
        topTags = try c.decodeIfPresent([RankedEntity].self, forKey: .topTags) ?? []
        genreShare = try c.decodeIfPresent([GenreShare].self, forKey: .genreShare) ?? []
        genreTaggedBooks = try c.decodeIfPresent(Int64.self, forKey: .genreTaggedBooks) ?? 0
        finishedBooks = try c.decodeIfPresent([FinishedBook].self, forKey: .finishedBooks) ?? []
        booksPerMonth = try c.decodeIfPresent([MonthCount].self, forKey: .booksPerMonth) ?? []
        ratingHistogram =
            try c.decodeIfPresent([RatingBucket].self, forKey: .ratingHistogram) ?? []
        pagesRead = try c.decodeIfPresent(Int64.self, forKey: .pagesRead)
        pagesPerHour = try c.decodeIfPresent(Double.self, forKey: .pagesPerHour)
        lengthBuckets = try c.decodeIfPresent([LengthBucket].self, forKey: .lengthBuckets) ?? []
        hourOfDay = try c.decodeIfPresent([HourBucket].self, forKey: .hourOfDay) ?? []
        dayOfWeek = try c.decodeIfPresent([WeekdayBucket].self, forKey: .dayOfWeek) ?? []
        unzonedSeconds = try c.decodeIfPresent(Int64.self, forKey: .unzonedSeconds) ?? 0
        superlatives = try c.decodeIfPresent(Superlatives.self, forKey: .superlatives)
            ?? Superlatives()
        goal = try c.decodeIfPresent(ReadingGoal.self, forKey: .goal)
        dailyGoals = try c.decodeIfPresent(DailyGoals.self, forKey: .dailyGoals) ?? DailyGoals()
        previous =
            try c.decodeIfPresent(PeriodComparison.self, forKey: .previous) ?? PeriodComparison()
        pagesDetail =
            try c.decodeIfPresent(PagesReadDetail.self, forKey: .pagesDetail) ?? PagesReadDetail()
    }

    var totalSeconds: Int64 { readingSeconds + listeningSeconds }

    /// Whether the time-pattern strips have anything to draw.
    ///
    /// Both are zero-filled to a fixed width, so "no data" and "a full day of
    /// nothing" render identically — this tells them apart. Mirrors
    /// `StatsSummary::has_time_patterns` in `omnibus_shared`, and checks the
    /// hour strip alone for the same reason: one rollup feeds both, so a
    /// populated weekday strip without a populated hour strip is not a state
    /// the server can produce.
    var hasTimePatterns: Bool { hourOfDay.contains { $0.seconds > 0 } }
}

/// The reader's target for one calendar year and their progress toward it.
///
/// `current` counts distinct books finished inside `year` under the same
/// definition as the Books-finished tile, so the two never disagree; it may
/// exceed `target`, which is the good case and is never clamped away.
struct ReadingGoal: Codable, Hashable, Sendable {
    var kind: String = "books"
    var target: Int64 = 0
    var current: Int64 = 0
    var year: Int64 = 0

    /// Progress as a 0...1 fraction for the ring, clamped. Read `current`
    /// against `target` for the honest ratio.
    var fraction: Double {
        guard target > 0 else { return 0 }
        return min(1, Double(current) / Double(target))
    }

    var isMet: Bool { current >= target }
    var remaining: Int64 { max(0, target - current) }
}

/// Write payload for `PUT /api/stats/goal`. `year` and `kind` are omitted so
/// the server names the calendar year; a `nil` `target` clears the goal.
struct ReadingGoalUpdate: Codable, Sendable {
    var target: Int64?

    enum CodingKeys: String, CodingKey { case target }

    /// Encoded explicitly: Swift's synthesized encoder drops a `nil`
    /// `Optional` key entirely, and while the server treats an absent
    /// `target` as a clear too, sending `null` states the intent on the wire.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(target, forKey: .target)
    }
}

/// A daily goal's kind — the two values `DailyGoalUpdate.kind` accepts.
///
/// Its own type rather than the raw string the wire carries, so a call site
/// cannot ask for a kind the server would 400 on. Mirrors
/// `omnibus_shared::{GOAL_KIND_PAGES, GOAL_KIND_MINUTES}` and the per-kind
/// bounds beside them; `books` has no daily analogue and so is absent.
enum DailyGoalKind: String, CaseIterable, Codable, Sendable, Identifiable {
    case pages
    case minutes

    var id: String { rawValue }

    /// How the editor names the target: "Pages a day".
    var label: String {
        switch self {
        case .pages: "Pages a day"
        case .minutes: "Minutes a day"
        }
    }

    /// How the goal card names the row beside its ring, where "a day" is
    /// already carried by the card's own heading.
    var shortLabel: String {
        switch self {
        case .pages: "Pages"
        case .minutes: "Minutes"
        }
    }

    /// The unit, singular. Error and remaining copy pluralises it.
    var unit: String {
        switch self {
        case .pages: "page"
        case .minutes: "minute"
        }
    }

    /// `MAX_DAILY_PAGES` / `MAX_DAILY_MINUTES`. Per-kind because the units
    /// are: 2,000 is a generous day of pages and an impossible day of minutes.
    var maxTarget: Int64 {
        switch self {
        case .pages: 2_000
        case .minutes: 1_440
        }
    }
}

/// One standing daily goal and today's progress toward it.
///
/// Recurring rather than year-bound, so there is no year here — the target
/// stands until it is changed, and `day` names the day `current` was measured
/// over. That day is **kind-dependent**: minutes are measured over the
/// reader's local day (off each session's capture-time offset), pages over the
/// UTC day the forward-progress ledger buckets to. The two can therefore name
/// different days for the same moment, which is why the field is per-goal.
struct DailyGoal: Codable, Hashable, Sendable {
    var kind: String = ""
    var target: Int64 = 0
    var current: Int64 = 0
    var day: String = ""

    /// Progress as a 0...1 fraction for the ring, clamped. Read `current`
    /// against `target` for the honest ratio — an over-target day is the good
    /// case and the figure never hides it.
    var fraction: Double {
        guard target > 0 else { return 0 }
        return min(1, Double(current) / Double(target))
    }

    var isMet: Bool { current >= target }
    /// Pages or minutes still to go, `0` once met or passed.
    var remaining: Int64 { max(0, target - current) }
    /// How far past the target the day went, `0` when it hasn't been reached.
    var over: Int64 { max(0, current - target) }
}

/// The reader's daily goals — at most one per kind, each independent of the
/// other and of the annual `ReadingGoal`.
struct DailyGoals: Codable, Sendable {
    var pages: DailyGoal?
    var minutes: DailyGoal?
    /// Seconds recorded today by sessions carrying no capture-time offset,
    /// which the minutes goal therefore could not place on a local day. Always
    /// `0` when no minutes goal is set — there is nothing to disclose against.
    var unzonedSeconds: Int64 = 0

    enum CodingKeys: String, CodingKey {
        case pages, minutes
        case unzonedSeconds = "unzoned_seconds"
    }

    init() {}

    /// Field-by-field for the same reason `StatsSummary` is: the synthesized
    /// decoder ignores property defaults, so one missing key would throw — and
    /// this whole object is absent from every server older than the daily-goal
    /// migration.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        pages = try c.decodeIfPresent(DailyGoal.self, forKey: .pages)
        minutes = try c.decodeIfPresent(DailyGoal.self, forKey: .minutes)
        unzonedSeconds = try c.decodeIfPresent(Int64.self, forKey: .unzonedSeconds) ?? 0
    }

    subscript(kind: DailyGoalKind) -> DailyGoal? {
        switch kind {
        case .pages: pages
        case .minutes: minutes
        }
    }

    /// Whether the reader has any daily goal at all — what turns the card's
    /// heading from "Daily goals" into "Today".
    var isEmpty: Bool { pages == nil && minutes == nil }
}

/// Write payload for `PUT /api/stats/goal/daily`. A `nil` `target` clears that
/// kind, leaving the other and the annual goal alone.
struct DailyGoalUpdate: Codable, Sendable {
    var kind: DailyGoalKind
    var target: Int64?

    enum CodingKeys: String, CodingKey { case kind, target }

    /// Encoded explicitly, exactly as `ReadingGoalUpdate` is: the synthesized
    /// encoder drops a `nil` `Optional` key entirely, and sending `null`
    /// states the clear on the wire rather than leaning on an absent key.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(kind, forKey: .kind)
        try c.encode(target, forKey: .target)
    }
}

/// The immediately preceding window's aggregates — the baseline the windowed
/// tiles render their deltas against.
///
/// Windowed like the figures it is compared to: switching the period moves
/// both sides together, so a delta always compares like with like.
struct PeriodComparison: Codable, Sendable {
    var booksFinished: Int64 = 0
    var avgStars: Double?
    var listeningSeconds: Int64 = 0
    /// Pages over the baseline window. Day-grained, unlike its siblings — the
    /// ledger buckets by UTC day — so the baseline includes the whole of its
    /// boundary day, matching the current window's own partial today.
    var pagesRead: Int64 = 0

    enum CodingKeys: String, CodingKey {
        case booksFinished = "books_finished"
        case avgStars = "avg_stars"
        case listeningSeconds = "listening_seconds"
        case pagesRead = "pages_read"
    }

    init() {}

    /// Field-by-field for the same reason `StatsSummary` is.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        booksFinished = try c.decodeIfPresent(Int64.self, forKey: .booksFinished) ?? 0
        avgStars = try c.decodeIfPresent(Double.self, forKey: .avgStars)
        listeningSeconds = try c.decodeIfPresent(Int64.self, forKey: .listeningSeconds) ?? 0
        pagesRead = try c.decodeIfPresent(Int64.self, forKey: .pagesRead) ?? 0
    }
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
///
/// The check-in flow has three front doors and they are not interchangeable:
/// a barcode read by the camera is `scan`, an ISBN typed by hand is `manual`,
/// and a title search is `search` — there any ISBN came from the provider,
/// because the reader supplied a title rather than a number.
enum WishlistSource: String, Codable, Sendable {
    case scan, detail, manual, search
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

// MARK: - Cross-format sync

enum CrossFormatResumeState: String, Codable, Sendable {
    case notLinked = "not_linked"
    case linkStale = "link_stale"
    case nothingNewer = "nothing_newer"
    case candidate
    /// Both formats already sit within the server's equivalence tolerance:
    /// the candidate rides along for navigation affordances, but prompt
    /// surfaces stay quiet.
    case aligned
}

enum MappingConfidence: String, Codable, Sendable {
    case linear
    case chapterAnchored = "chapter_anchored"
    case userAnchored = "user_anchored"
}

enum CrossFormatLinkMode: String, Codable, Sendable {
    case sequence
    case narrations
}

struct CrossFormatCandidate: Codable, Hashable, Sendable {
    var target: ProgressFormat
    var sourceFormat: ProgressFormat
    var sourceClientUpdatedAt: Int64
    var confidence: MappingConfidence
    var bookFileID: Int64?
    var audioPositionSeconds: Double?
    var totalDurationSeconds: Double?
    var percent: Int64?
    /// Full-precision jump fraction (0...1) on epub targets; `percent` is
    /// its floored display twin.
    var fraction: Double?
    /// `false` marks a backward offer (the source regressed) — copy must
    /// not claim "further". Absent means ahead.
    var sourceAhead: Bool?

    enum CodingKeys: String, CodingKey {
        case target
        case sourceFormat = "source_format"
        case sourceClientUpdatedAt = "source_client_updated_at"
        case confidence
        case bookFileID = "book_file_id"
        case audioPositionSeconds = "audio_position_seconds"
        case totalDurationSeconds = "total_duration_seconds"
        case percent
        case fraction
        case sourceAhead = "source_ahead"
    }
}

struct CrossFormatResume: Codable, Hashable, Sendable {
    var state: CrossFormatResumeState
    var candidate: CrossFormatCandidate?
    /// Follow mode: apply a candidate silently instead of offering.
    var follow: Bool?
}

struct AlignmentLink: Codable, Hashable, Sendable {
    var mode: CrossFormatLinkMode
    var primaryBookFileID: Int64?
    var stale: Bool
    var confirmedAt: Int64
    var follow: Bool?
    var userAnchors: Int64?

    enum CodingKeys: String, CodingKey {
        case mode
        case primaryBookFileID = "primary_book_file_id"
        case stale
        case confirmedAt = "confirmed_at"
        case follow
        case userAnchors = "user_anchors"
    }
}

/// Body of `POST /api/books/{uuid}/sync-point` — the declaring surface
/// names its own position; the counterpart comes from the server's row.
struct DeclareSyncPoint: Codable, Sendable {
    var bookUUID: String
    var format: ProgressFormat
    var ebookFraction: Double?
    var audioBookFileID: Int64?
    var audioSeconds: Double?

    enum CodingKeys: String, CodingKey {
        case bookUUID = "book_uuid"
        case format
        case ebookFraction = "ebook_fraction"
        case audioBookFileID = "audio_book_file_id"
        case audioSeconds = "audio_seconds"
    }
}

struct AlignmentMatch: Codable, Hashable, Sendable {
    var matched: Int64
    var ebookChapters: Int64
    var confidence: MappingConfidence

    enum CodingKeys: String, CodingKey {
        case matched
        case ebookChapters = "ebook_chapters"
        case confidence
    }
}

struct AlignmentEbookChapter: Codable, Hashable, Sendable {
    var title: String
    var percent: Double
}

struct AlignmentEbook: Codable, Hashable, Sendable {
    var totalChars: Int64
    var chapters: [AlignmentEbookChapter]

    enum CodingKeys: String, CodingKey {
        case totalChars = "total_chars"
        case chapters
    }
}

struct AlignmentAudioFile: Codable, Hashable, Sendable, Identifiable {
    var bookFileID: Int64
    var label: String
    var durationSeconds: Double
    var chapterStarts: [Double]

    var id: Int64 { bookFileID }

    enum CodingKeys: String, CodingKey {
        case bookFileID = "book_file_id"
        case label
        case durationSeconds = "duration_seconds"
        case chapterStarts = "chapter_starts"
    }
}

struct AlignmentPosition: Codable, Hashable, Sendable {
    var percent: Int64?
    var clientUpdatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case percent
        case clientUpdatedAt = "client_updated_at"
    }
}

struct AlignmentAudioPosition: Codable, Hashable, Sendable {
    var bookFileID: Int64?
    var seconds: Double
    var clientUpdatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case bookFileID = "book_file_id"
        case seconds
        case clientUpdatedAt = "client_updated_at"
    }
}

struct AlignmentView: Codable, Hashable, Sendable {
    var link: AlignmentLink?
    var anchorMatch: AlignmentMatch?
    var ebook: AlignmentEbook?
    var audioFiles: [AlignmentAudioFile]
    var reading: AlignmentPosition?
    var listening: AlignmentAudioPosition?
    /// Usable audio chapter marks: with no anchor match, zero means "no
    /// marks" and nonzero means "marks exist but couldn't be aligned".
    var audioChapterMarks: Int64?
    /// Matched anchor pairs (text_frac, audio_frac) — the mapped-preview
    /// interpolation, identical to the pairs the jump uses. Decoded via
    /// `decodeIfPresent`: synthesized Codable ignores the `= []` default
    /// and throws `keyNotFound` on an absent key, and older servers omit
    /// the key for linear-tier books.
    var anchorPairs: [[Double]] = []

    enum CodingKeys: String, CodingKey {
        case link
        case anchorMatch = "anchor_match"
        case ebook
        case audioFiles = "audio_files"
        case reading
        case listening
        case audioChapterMarks = "audio_chapter_marks"
        case anchorPairs = "anchor_pairs"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        link = try c.decodeIfPresent(AlignmentLink.self, forKey: .link)
        anchorMatch = try c.decodeIfPresent(AlignmentMatch.self, forKey: .anchorMatch)
        ebook = try c.decodeIfPresent(AlignmentEbook.self, forKey: .ebook)
        audioFiles = try c.decode([AlignmentAudioFile].self, forKey: .audioFiles)
        reading = try c.decodeIfPresent(AlignmentPosition.self, forKey: .reading)
        listening = try c.decodeIfPresent(AlignmentAudioPosition.self, forKey: .listening)
        audioChapterMarks = try c.decodeIfPresent(Int64.self, forKey: .audioChapterMarks)
        anchorPairs = try c.decodeIfPresent([[Double]].self, forKey: .anchorPairs) ?? []
    }
}

struct ConfirmCrossFormatLink: Codable, Sendable {
    var bookUUID: String
    var mode: CrossFormatLinkMode
    var primaryBookFileID: Int64?
    var audioOrder: [Int64]?

    enum CodingKeys: String, CodingKey {
        case bookUUID = "book_uuid"
        case mode
        case primaryBookFileID = "primary_book_file_id"
        case audioOrder = "audio_order"
    }
}

// MARK: - Book upload

/// Metadata the server read out of an uploaded EPUB, for the editable confirm
/// step. Mirrors `omnibus_shared::UploadInspection`.
struct UploadInspection: Codable, Hashable, Sendable {
    var title: String?
    var author: String?
    var series: String?
    var seriesIndex: String?
    var language: String?
    var hasCover: Bool
    /// Lowercased extension the server settled on (`"epub"`).
    var ext: String

    enum CodingKeys: String, CodingKey {
        case title, author, series, language, ext
        case seriesIndex = "series_index"
        case hasCover = "has_cover"
    }
}

/// Tag-derived metadata for an uploaded audiobook — the audiobook sibling of
/// [`UploadInspection`]. Mirrors `omnibus_shared::AudiobookInspection`.
struct AudiobookInspection: Codable, Hashable, Sendable {
    var title: String?
    var author: String?
    var hasCover: Bool
    /// Lowercased format the server settled on (`"m4b"`, `"m4a"`, `"mp3"`).
    var format: String
    /// Files in the upload — 1 for a container, N for a multi-part MP3 book.
    var partCount: Int
    var durationSeconds: Double?

    enum CodingKeys: String, CodingKey {
        case title, author, format
        case hasCover = "has_cover"
        case partCount = "part_count"
        case durationSeconds = "duration_seconds"
    }
}

/// The uuid of a freshly filed book, so the client can link straight to it.
/// Mirrors `omnibus_shared::UploadCommitResult`.
struct UploadCommitResult: Codable, Hashable, Sendable {
    var uuid: String
}

// MARK: - Reading session log

/// Which of the two session tables a stitched sitting drew from. Mirrors
/// `omnibus_shared::SessionFormat`.
///
/// `mixed` is not a fallback: a dual-format book read and listened to in one
/// stretch stitches into a single sitting server-side, and naming it as one
/// format alone would claim time the reader didn't spend there.
enum SessionFormat: String, Codable, Hashable, Sendable {
    case reading
    case listening
    case mixed

    /// Past tense — a logged sitting is over.
    var label: String {
        switch self {
        case .reading: "Read"
        case .listening: "Listened"
        case .mixed: "Read & listened"
        }
    }
}

/// One sitting in the reading-session log — adjacent checkpoint rows stitched
/// back together, so a row is a sit rather than a heartbeat flush. Mirrors
/// `omnibus_shared::SessionLogEntry`.
///
/// `seconds` is the *recorded* time, not `endedAt - startedAt`: a sitting the
/// reader paused mid-way spans more wall clock than it recorded, and the
/// recorded figure is what every other stats surface reports.
struct SessionLogEntry: Codable, Hashable, Sendable, Identifiable {
    var bookUUID: String
    var title: String
    var format: SessionFormat
    var startedAt: Int64
    var endedAt: Int64
    var seconds: Int64

    /// `(book, start)` is unique per sitting — two sittings of one book cannot
    /// share a start.
    var id: String { bookUUID + ":" + String(startedAt) }

    enum CodingKeys: String, CodingKey {
        case title, format, seconds
        case bookUUID = "book_uuid"
        case startedAt = "started_at"
        case endedAt = "ended_at"
    }
}

/// One page of the session log, newest first. Mirrors
/// `omnibus_shared::SessionLogPage`.
///
/// `nextBefore` is an opaque keyset cursor: echo it back as `before` to get the
/// page that continues after the last entry here, `nil` at the end of the log.
/// Not an offset — a sitting landing mid-scroll would shift every later page.
struct SessionLogPage: Codable, Hashable, Sendable {
    var entries: [SessionLogEntry]
    var nextBefore: String?

    enum CodingKeys: String, CodingKey {
        case entries
        case nextBefore = "next_before"
    }
}
