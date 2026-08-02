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
    var series = ""
    var seriesIndex = ""
    var publisher = ""
    var published = ""
    var language = ""
    var isbn13 = ""
    var description = ""

    init() {}

    /// Snapshot of a book's current effective metadata — the baseline the
    /// per-field edited markers and the save diff compare against.
    init(book: Book) {
        title = book.title ?? ""
        authors = book.creators.map(\.name)
        tags = book.subjects
        series = book.series ?? ""
        seriesIndex = book.seriesIndex ?? ""
        publisher = book.publisher ?? ""
        published = book.published ?? ""
        language = book.language ?? ""
        isbn13 = book.isbn13 ?? ""
        description = book.description ?? ""
    }

    /// The changed-fields-only body for `POST /api/ebooks/{uuid}/overrides`.
    ///
    /// Sending every field on every save wrote overrides for fields nobody
    /// touched, pinning scanned values so a later rescan could no longer
    /// update them. The endpoint merges, so omitting a field leaves it as it
    /// was, and an empty string clears an existing override.
    func payload(since loaded: MetadataDraft) -> MetadataOverridesPayload {
        func changed(_ key: KeyPath<MetadataDraft, String>) -> String? {
            self[keyPath: key] == loaded[keyPath: key] ? nil : self[keyPath: key]
        }
        return MetadataOverridesPayload(
            title: changed(\.title),
            creators: authors == loaded.authors
                ? nil : authors.map(MetadataOverridesPayload.Creator.init),
            subjects: tags == loaded.tags ? nil : tags,
            series: changed(\.series),
            series_index: changed(\.seriesIndex),
            publisher: changed(\.publisher),
            published: changed(\.published),
            language: changed(\.language),
            isbn13: changed(\.isbn13),
            description: changed(\.description)
        )
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
    var series: String?
    var series_index: String?
    var publisher: String?
    var published: String?
    var language: String?
    var isbn13: String?
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
        let trimmed = entry.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }
        if deduplicating {
            let lowered = trimmed.lowercased()
            guard !existing.contains(where: { $0.lowercased() == lowered }) else { return nil }
        }
        return trimmed
    }
}
