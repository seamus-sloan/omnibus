//  MetadataDraft.swift
//  The metadata editor's editable snapshot and its wire payload: one
//  comparable value for "has anything changed", plus the changed-fields-only
//  overrides body a save posts. Kept out of the view so the diff and
//  chip-commit logic are testable.

import Foundation

/// Every editable value on the editor screen, as one comparable value.
///
/// Kept as a single struct rather than ten `@State` strings so "has anything
/// changed" is a plain `!=` against the loaded snapshot — which is what lets
/// Save disable itself until there's something to save, and what drives the
/// per-field edited markers.
struct MetadataDraft: Equatable {
    var title = ""
    /// A real list, not a comma-joined string: authors are structured on the
    /// wire, and asking someone to maintain delimiters by hand made the one
    /// genuinely multi-value field the most error-prone on the screen.
    var authors: [String] = []
    /// Tags (the server's `subjects`), as the same chip list authors use.
    var tags: [String] = []
    /// Genres, as a second chip list. Separate from `tags`: nothing the
    /// server parses carries a genre, so this list exists only because
    /// someone assigned it.
    var genres: [String] = []
    var series = ""
    var seriesIndex = ""
    var publisher = ""
    var published = ""
    var language = ""
    var isbn13 = ""
    var isbn10 = ""
    /// Held as text, not `Int64?`, because that is what a text field edits —
    /// the parse to the wire's integer happens once, in `payload(since:)`.
    var printPages = ""
    var description = ""

    init() {}

    /// Snapshot of a book's current effective metadata — the baseline the
    /// per-field edited markers and the save diff compare against.
    init(book: Book) {
        title = book.title ?? ""
        authors = book.creators.map(\.name)
        tags = book.subjects
        genres = book.genres
        series = book.series ?? ""
        seriesIndex = book.seriesIndex ?? ""
        publisher = book.publisher ?? ""
        published = book.published ?? ""
        language = book.language ?? ""
        isbn13 = book.isbn13 ?? ""
        isbn10 = book.isbn10 ?? ""
        printPages = book.printPages.map(String.init) ?? ""
        description = book.description ?? ""
    }

    /// The changed-fields-only body for `POST /api/ebooks/{uuid}/overrides`.
    ///
    /// Sending every field on every save wrote overrides for fields nobody
    /// touched, pinning scanned values so a later rescan could no longer
    /// update them. The endpoint merges, so omitting a field leaves it as it
    /// was, and an empty string clears an existing override.
    ///
    /// Throws `MetadataDraftError` when the print-pages field holds something
    /// that isn't a whole number — rejected here so the editor can say so
    /// instead of posting a body the server would 400.
    func payload(since loaded: MetadataDraft) throws -> MetadataOverridesPayload {
        func changed(_ key: KeyPath<MetadataDraft, String>) -> String? {
            self[keyPath: key] == loaded[keyPath: key] ? nil : self[keyPath: key]
        }
        return MetadataOverridesPayload(
            title: changed(\.title),
            creators: authors == loaded.authors
                ? nil : authors.map(MetadataOverridesPayload.Creator.init),
            subjects: tags == loaded.tags ? nil : tags,
            genres: genres == loaded.genres ? nil : genres,
            series: changed(\.series),
            series_index: changed(\.seriesIndex),
            publisher: changed(\.publisher),
            published: changed(\.published),
            language: changed(\.language),
            isbn13: changed(\.isbn13),
            isbn10: changed(\.isbn10),
            print_pages: try changedPrintPages(since: loaded),
            description: changed(\.description)
        )
    }

    /// The print-pages value a save sends, or `nil` to omit the field.
    ///
    /// Unlike every scalar above, this one can't be diffed as a raw string:
    /// the wire field is an integer, and there is no "empty" integer to carry
    /// the empty-string-clears convention. So a blanked field means *leave the
    /// override alone*, not *clear it* — matching the web editor
    /// (`build_overrides` in `frontend/src/pages/metadata_edit.rs`), which the
    /// two editors have to agree on.
    private func changedPrintPages(since loaded: MetadataDraft) throws -> Int64? {
        guard let parsed = try MetadataDraft.parsePrintPages(printPages) else { return nil }
        return parsed == loaded.printPagesValue ? nil : parsed
    }

    /// This draft's print-pages field as an integer, ignoring a blank or
    /// unparseable entry. Only meaningful on a `loaded` snapshot, whose value
    /// came from the server and so always parses.
    private var printPagesValue: Int64? {
        try? MetadataDraft.parsePrintPages(printPages)
    }

    /// Parses the print-pages field: blank is `nil` (nothing to send), and a
    /// non-numeric entry is rejected with a specific message rather than
    /// silently dropped, which would look like a save that worked.
    static func parsePrintPages(_ input: String) throws -> Int64? {
        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        guard let value = Int64(trimmed) else {
            throw MetadataDraftError.invalidPrintPages
        }
        return value
    }
}

/// Client-side rejections raised before a save leaves the device.
enum MetadataDraftError: LocalizedError, Equatable {
    case invalidPrintPages

    var errorDescription: String? {
        switch self {
        case .invalidPrintPages: "Print page count must be a whole number."
        }
    }
}

/// Body for `POST /api/ebooks/{uuid}/overrides`. Field names match the wire.
struct MetadataOverridesPayload: Encodable, Equatable {
    /// `creators` is a list of Contributor *objects* on the wire, not bare
    /// strings: sending `["E. M. Forster"]` fails the server's JSON
    /// extraction outright, so every save with a non-empty Authors field
    /// was rejected before it reached validation.
    struct Creator: Encodable, Equatable {
        var name: String
    }

    var title: String?
    var creators: [Creator]?
    /// Replaces the whole tag list when present — the server's `subjects`
    /// override is a wholesale replace, never an append.
    var subjects: [String]?
    /// Replaces the whole genre list when present, same wholesale semantics
    /// as `subjects`.
    var genres: [String]?
    var series: String?
    var series_index: String?
    var publisher: String?
    var published: String?
    var language: String?
    var isbn13: String?
    var isbn10: String?
    /// Snake-cased to match the wire, like `series_index`. Absent unless the
    /// field actually changed — it has no empty-string-clears form.
    var print_pages: Int64?
    var description: String?
}

/// Chip-entry commit semantics shared by the authors and tags fields — and by
/// the save path, which flushes a value still sitting in an entry field.
enum ChipEntry {
    /// The chip an entry commits: trimmed, or `nil` when blank. Deduplicating
    /// fields also refuse a value already present ignoring case, matching the
    /// web chip editor — a duplicate tag means nothing, while two credited
    /// contributors can legitimately share a name.
    static func committed(from entry: String, existing: [String], deduplicating: Bool) -> String? {
        let trimmed = entry.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if deduplicating {
            let lowered = trimmed.lowercased()
            guard !existing.contains(where: { $0.lowercased() == lowered }) else { return nil }
        }
        return trimmed
    }
}
