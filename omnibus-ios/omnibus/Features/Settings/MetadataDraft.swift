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

    /// Field checks that don't need the server, run by `save` *before* it
    /// touches any state — the web editor rejects the same entries in
    /// `build_on_save`'s first statement, ahead of its own `spawn`, for the
    /// reason this ordering matters: a rejected save must leave the form
    /// exactly as the user left it.
    ///
    /// The checks mirror `MetadataOverrides::validate` rather than
    /// duplicating the whole of it. What's here is what the server would
    /// reject for these two fields, caught before a round trip that would
    /// take every *other* edit in the same body down with it.
    func validate(since loaded: MetadataDraft) throws {
        if printPages.nilIfBlank == nil, loaded.printPagesValue != nil {
            throw MetadataDraftError.printPagesNotClearable
        }
        _ = try MetadataDraft.parsePrintPages(printPages)
        try MetadataDraft.validateIsbn10(isbn10)
    }

    /// The changed-fields-only body for `POST /api/ebooks/{uuid}/overrides`.
    ///
    /// Sending every field on every save wrote overrides for fields nobody
    /// touched, pinning scanned values so a later rescan could no longer
    /// update them. The endpoint merges, so omitting a field leaves it as it
    /// was, and an empty string clears an existing override.
    ///
    /// Total, deliberately: this is a diff between two snapshots, and
    /// rejecting user input is `validate(since:)`'s job. An entry this can't
    /// use is simply not sent — which is safe only because `save` validates
    /// first, and is why `MetadataOverridesPayload.isEmpty` exists to catch a
    /// body with nothing in it.
    func payload(since loaded: MetadataDraft) -> MetadataOverridesPayload {
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
            isbn10: changedIsbn10(since: loaded),
            print_pages: changedPrintPages(since: loaded),
            description: changed(\.description)
        )
    }

    /// ISBN-10 diffed on its *trimmed* form. The field is `.asciiCapable`
    /// (an ISBN-10's check digit can be `X`), so unlike the `.numberPad`
    /// ISBN-13 beside it a pasted value can carry surrounding whitespace —
    /// which the server rejects rather than strips.
    private func changedIsbn10(since loaded: MetadataDraft) -> String? {
        // `?? ""` keeps the empty-string-clears sentinel: a blanked field
        // still has to reach the server as `""` to clear the override.
        let value = isbn10.nilIfBlank ?? ""
        return value == (loaded.isbn10.nilIfBlank ?? "") ? nil : value
    }

    /// The print-pages value a save sends, or `nil` to omit the field.
    ///
    /// Unlike every scalar above, this one can't be diffed as a raw string:
    /// the wire field is an integer, and there is no "empty" integer to carry
    /// the empty-string-clears convention. So a blanked field means *leave the
    /// override alone*, not *clear it* — matching the web editor
    /// (`build_overrides` in `frontend/src/pages/metadata_edit.rs`), which the
    /// two editors have to agree on. `validate(since:)` refuses that blank up
    /// front so the gap surfaces as a message rather than a silent no-op.
    private func changedPrintPages(since loaded: MetadataDraft) -> Int64? {
        guard let parsed = printPagesValue else { return nil }
        return parsed == loaded.printPagesValue ? nil : parsed
    }

    /// This draft's print-pages field as an integer, or `nil` when blank or
    /// unusable. `validate(since:)` is what turns "unusable" into a message.
    private var printPagesValue: Int64? {
        try? MetadataDraft.parsePrintPages(printPages)
    }

    /// Parses the print-pages field: blank is `nil` (nothing to send), and
    /// anything the server would refuse is rejected here instead, so the
    /// reader gets the message without spending a round trip that would also
    /// discard every other edit in the same body.
    static func parsePrintPages(_ input: String) throws -> Int64? {
        guard let trimmed = input.nilIfBlank else { return nil }
        guard let value = Int64(trimmed) else {
            throw MetadataDraftError.invalidPrintPages
        }
        // Mirrors `MetadataOverrides::validate_print_pages`, whose bound is
        // `1..=PRINT_PAGES_MAX`. Checked here so "0" — one tap away on a
        // number pad — reads as the range error it is rather than as the
        // "not a number" one it isn't.
        guard (1...printPagesMax).contains(value) else {
            throw MetadataDraftError.printPagesOutOfRange
        }
        return value
    }

    /// `MetadataOverrides::PRINT_PAGES_MAX`. Duplicated rather than fetched:
    /// it is a compile-time constant on a type this app never sees, and the
    /// server still enforces it if the two ever drift.
    static let printPagesMax: Int64 = 20_000

    /// ISBN-10 shape, mirroring `MetadataOverrides::validate_isbn10`: blank
    /// (which clears), or 10 characters — 9 digits plus a check digit that
    /// may be `X`. Format only, no mod-11 check, matching the server's
    /// reasoning that this is typed metadata rather than a scanned barcode.
    static func validateIsbn10(_ input: String) throws {
        guard let trimmed = input.nilIfBlank else { return }
        let shapeOK = trimmed.count == 10
            && trimmed.prefix(9).allSatisfy(\.isASCIIDigit)
            && trimmed.last.map { $0.isASCIIDigit || $0 == "X" || $0 == "x" } == true
        guard shapeOK else { throw MetadataDraftError.invalidIsbn10 }
    }
}

/// Client-side rejections raised before a save leaves the device. Each one
/// is a check the server also makes — caught here so the reader reads it
/// immediately, and so a bad entry in one field doesn't take the rest of
/// the form's edits down with it in a rejected body.
enum MetadataDraftError: LocalizedError, Equatable {
    case invalidPrintPages
    case printPagesOutOfRange
    case printPagesNotClearable
    case invalidIsbn10

    var errorDescription: String? {
        switch self {
        case .invalidPrintPages:
            "Print page count must be a whole number."
        case .printPagesOutOfRange:
            "Print page count must be between 1 and \(MetadataDraft.printPagesMax)."
        case .printPagesNotClearable:
            """
            Print page count can't be cleared once set. Enter a number, or use \
            Revert to scanned metadata to remove every override on this book.
            """
        case .invalidIsbn10:
            "ISBN-10 must be 10 characters: 9 digits plus a digit or X check digit."
        }
    }
}

private extension Character {
    /// `Character.isNumber` is true for "٣" and "Ⅶ"; the server's check is
    /// `is_ascii_digit`, and an ISBN that passes here must pass there.
    var isASCIIDigit: Bool { isASCII && isNumber }
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

    /// Whether this body would say nothing at all.
    ///
    /// Worth a guard because `{}` is not a no-op server-side: with no prior
    /// row, `merge_one_in_tx` takes its `None` branch and *inserts* an empty
    /// override, which makes `apply_overrides` report `has_override = true`
    /// forever — the book reads "Edited" with nothing to revert, and it
    /// leaves the byte-faithful passthrough branch for EPUB downloads. The
    /// same write also bumps `books.last_modified`, the invalidation clock
    /// for the thumbnail, KEPUB, and export-EPUB caches. `clear_cover_override`
    /// already reaps such rows for exactly this reason ("an empty-but-present
    /// row would leave the 'Override active' indicator stuck on"); the merge
    /// path has no equivalent, so the client must not create one.
    ///
    /// Reachable whenever a field is edited to a value that normalizes back
    /// to what was loaded — retyping `180` as `0180`, or adding a stray
    /// space — since the editor's dirty check compares raw text.
    var isEmpty: Bool { self == MetadataOverridesPayload() }
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
