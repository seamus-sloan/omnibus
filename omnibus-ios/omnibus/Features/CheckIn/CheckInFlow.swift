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
