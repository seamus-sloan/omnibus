//  CheckInFlow.swift
//  The check-in flow's stage machine and its pure decision logic — what screen
//  follows each outcome, and what the success screen shows. Mirrors the web
//  client's `Stage` enum so both clients resolve a scan the same way.

import Foundation

/// One screen of the check-in sheet. Success is a *stage*, not an annotation
/// on the outcome screen — once a write lands, the action buttons are gone,
/// so a second tap (and a duplicate row) is unrepresentable.
enum CheckInStage: Equatable {
    case scan
    case outcome(ScanOutcome)
    /// Picking the library book a scanned copy belongs to, when no rung of the
    /// matching ladder could reach it. Carries the ISBN the copy is filed
    /// under and the outcome screen Back returns to.
    case linkExisting(isbn: String?, returnTo: ScanOutcome)
    case success(CheckInSuccess)
}

/// Everything the terminal success screen needs to render.
struct CheckInSuccess: Equatable {
    enum Tone: Equatable {
        /// Owning a book is the joyous occasion — confetti.
        case celebration
        /// Wishlisting just tracks it — rings only.
        case quiet
    }

    /// Where the cover art comes from: the library thumbnail pipeline, a
    /// provider-hosted URL, or the lettered fallback plate.
    enum Cover: Equatable {
        case library(uuid: String)
        case external(url: String)
        case plate
    }

    var tone: Tone
    var headline: String
    var title: String
    /// "View book" target; nil hides the button.
    var bookUUID: String?
    var cover: Cover
}

/// Pure builders and gates, separated from the view so they're testable
/// without a server (same pattern as `LibraryModel.shouldPoll`).
enum CheckInFlow {
    /// Checked in a physical copy of a library book. The server answers with
    /// the *canonical* uuid (merged books resolve to their primary), so the
    /// success screen links and renders through `ref`, not the scanned book.
    static func checkedInSuccess(book: ScanBook, ref: BookRef) -> CheckInSuccess {
        CheckInSuccess(
            tone: .celebration,
            headline: "In your physical collection",
            title: book.title,
            bookUUID: ref.bookUUID,
            cover: .library(uuid: ref.bookUUID)
        )
    }

    /// Added a physical-only book that wasn't in the library. The book is
    /// brand new, so its server cover may not exist yet — prefer the
    /// provider's art over the freshly minted uuid.
    static func addedSuccess(meta: ExternalBookMeta, ref: BookRef) -> CheckInSuccess {
        CheckInSuccess(
            tone: .celebration,
            headline: "In your physical collection",
            title: meta.title,
            bookUUID: ref.bookUUID,
            cover: externalCover(meta)
        )
    }

    /// Wishlisted a book that wasn't in the library.
    static func wishlistedSuccess(meta: ExternalBookMeta, ref: BookRef) -> CheckInSuccess {
        CheckInSuccess(
            tone: .quiet,
            headline: "On your wishlist",
            title: meta.title,
            bookUUID: ref.bookUUID,
            cover: externalCover(meta)
        )
    }

    /// The edition note belongs on exactly one screen — the in-library
    /// check-in confirm — matching the web client.
    static func showsNoteField(for outcome: ScanOutcome) -> Bool {
        if case .inLibraryUnowned = outcome { return true }
        return false
    }

    /// A resolve fires from the scan stage's ISBN field, whose fallback
    /// screen — `.outcome(.unresolved)` — reuses the *same* title-search
    /// query/results state. Leaving `.scan` for any outcome must clear it,
    /// or the fallback opens showing a search from the book just resolved.
    static func resolveShouldClearSearch(from stage: CheckInStage) -> Bool {
        if case .scan = stage { return true }
        return false
    }

    /// A title search is a separate in-flight request that can outlive a
    /// resolve landing elsewhere (or a restart, or a newer keystroke) —
    /// its response must only apply if the stage and query it was asked
    /// with are still current, or a late answer reintroduces the same
    /// stale-search bug `resolveShouldClearSearch` guards against.
    ///
    /// Both sides are trimmed because only one of them ever was: the request
    /// carries `nilIfBlank`'s trimmed text while the field holds whatever was
    /// typed, and iOS appends a trailing space to every accepted QuickType
    /// suggestion. Comparing those raw discarded the answer to the search the
    /// reader had just asked for, leaving a spinner and no results.
    static func searchResponseShouldApply(
        startedStage: CheckInStage, currentStage: CheckInStage,
        startedQuery: String, currentQuery: String
    ) -> Bool {
        startedStage == currentStage
            && startedQuery.trimmingCharacters(in: .whitespacesAndNewlines)
            == currentQuery.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Outcomes that offer "I already have this book".
    ///
    /// Every leaf where the ladder failed to land on a library book the reader
    /// may nonetheless own: a print barcode the ebook doesn't publish, a
    /// provider record whose title shares nothing with the shelf copy (release
    /// placeholders are the usual culprit), or an ISBN no provider knows. The
    /// two screens that *did* find the book don't need it.
    static func offersLinkExisting(for outcome: ScanOutcome) -> Bool {
        switch outcome {
        case .closeMatch, .notInLibrary, .unresolved:
            return true
        case .alreadyOwned, .onWishlist, .inLibraryUnowned:
            return false
        }
    }

    /// The ISBN a hand-linked copy is filed under — always the one in the
    /// reader's hand, never one the picked library book happens to publish.
    /// The unresolved screen has no record to read it from, so it falls back
    /// to whatever was scanned or typed.
    static func linkISBN(for outcome: ScanOutcome, typed: String) -> String? {
        switch outcome {
        case let .notInLibrary(online):
            return online.isbn13.nilIfBlank
        case let .closeMatch(_, scanned):
            return scanned.isbn13.nilIfBlank
        case .unresolved:
            return typed.nilIfBlank
        case .alreadyOwned, .onWishlist, .inLibraryUnowned:
            return nil
        }
    }

    /// The library book a picker row stands for, carrying the scanned ISBN so
    /// the in-library confirm files the copy under it.
    ///
    /// `hasPhysical` stays `false`: the search projection carries no copy
    /// count, and nothing on this path reads it — the confirm screen shows the
    /// cover, title, and byline only, and the write resolves ownership
    /// server-side.
    static func linkTarget(from hit: PaletteBookHit, isbn: String?) -> ScanBook {
        ScanBook(
            uuid: hit.uuid,
            title: hit.title,
            authors: hit.authorDisplay
                .components(separatedBy: ", ")
                .compactMap { $0.nilIfBlank },
            coverURL: hit.coverURL,
            hasPhysical: false,
            isbn: isbn
        )
    }

    /// The line under a truncated picker list. The palette caps each category
    /// at five hits, so a broad query hides matches; say so rather than let the
    /// reader conclude their book isn't there. `nil` when nothing is hidden.
    static func linkTruncationNote(shown: Int, total: UInt32) -> String? {
        guard Int(total) > shown else { return nil }
        return "Showing \(shown) of \(total) matches — add the author or more of the title."
    }

    /// Outcomes whose card links straight to the book's detail page.
    static func detailUUID(for outcome: ScanOutcome) -> String? {
        switch outcome {
        case let .alreadyOwned(book), let .onWishlist(book):
            return book.uuid
        case .inLibraryUnowned, .closeMatch, .notInLibrary, .unresolved:
            return nil
        }
    }

    /// Provider cover URLs are absolute; the library pipeline's are
    /// server-relative. Only the former may be fetched unauthenticated.
    static func isExternalURL(_ path: String) -> Bool {
        path.hasPrefix("https://") || path.hasPrefix("http://")
    }

    /// The provider fact lines under an online book card: the series statement
    /// on its own line, then first-publish year / publisher / page count joined
    /// with dots — whichever the provider carried. Empty when it carried none,
    /// so a card renders no blank lines.
    static func detailLines(for meta: ExternalBookMeta) -> [String] {
        var lines: [String] = []
        if let series = meta.series?.nilIfBlank {
            lines.append(series)
        }
        var facts: [String] = []
        if let year = meta.firstPublishYear {
            facts.append("First published \(year)")
        }
        if let publisher = meta.publisher?.nilIfBlank {
            facts.append(publisher)
        }
        if let pages = meta.pages, pages > 0 {
            facts.append("\(pages) pages")
        }
        if !facts.isEmpty {
            lines.append(facts.joined(separator: " \u{b7} "))
        }
        return lines
    }

    private static func externalCover(_ meta: ExternalBookMeta) -> CheckInSuccess.Cover {
        if let url = meta.coverURL, isExternalURL(url) {
            return .external(url: url)
        }
        return .plate
    }
}
