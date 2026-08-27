//  WidgetSnapshot.swift
//  What the app hands the Home Screen, and the only thing a widget reads.
//
//  A timeline render gets a tight memory and time budget, so the extension is
//  given a finished answer rather than a second reader on `OfflineStore` — no
//  SQLite, no decoding of the full `Book` payloads, no network. Everything a
//  card draws is resolved app-side and written here.

import Foundation

/// The `kind` strings WidgetKit identifies each widget by. Shared so the app
/// can reload one precisely — `reloadAllTimelines` would redraw every widget
/// this bundle ever grows, for a snapshot only one of them reads.
enum WidgetKind {
    static let continueReading = "ContinueReadingWidget"
}

/// Which surface a position belongs to. Raw values match `ProgressFormat` in
/// the app, so a snapshot entry and a progress row name the same thing.
enum WidgetFormat: String, Codable, Sendable {
    case epub
    case audio
}

/// One card on the Continue widget.
struct WidgetBook: Codable, Sendable, Equatable, Identifiable {
    /// The book's colour in OKLCH, already resolved by `CoverIdentity.tone` —
    /// extracted cover accent, else the title-derived hue. Resolved app-side
    /// so the widget and the Continue hero cannot show the same book in two
    /// different colours, which is the failure `CoverIdentity` exists to
    /// prevent within the app.
    struct Tone: Codable, Sendable, Equatable {
        var l: Double
        var c: Double
        var h: Double
    }

    var bookUUID: String
    var format: WidgetFormat
    var title: String
    var author: String
    var tone: Tone
    /// Fraction complete, when the format has an honest one. Audio derives it
    /// from position over duration; a reading row has one only when it carries
    /// the cross-surface percent, so a CFI-only EPUB save leaves this `nil`.
    var fraction: Double?
    /// Wall-clock seconds left at the reader's saved speed. Audio only.
    var secondsRemaining: Double?
    /// When the position was last written — the large family's "last read".
    var updatedAt: Date
    /// The audiobook file the position was taken in, so a tap reopens that
    /// narration rather than the book's first one. Two narrations do not share
    /// a timeline.
    var fileID: Int64?
    /// Name of the pre-rendered cover inside `WidgetStore.thumbsDirectory`.
    /// `nil` when the book has no art, or none had reached the device.
    var thumb: String?

    /// Scoped by format as well as book, mirroring `ResumePoint.id`: a
    /// dual-format book someone is both reading and listening to is two cards.
    var id: String { "\(bookUUID):\(format.rawValue)" }
}

/// The whole of what the widget process is given.
struct WidgetSnapshot: Codable, Sendable, Equatable {
    /// Why `books` is empty, when it is. Kept apart from the list because the
    /// three empty cases want three different things on screen — "sign in",
    /// "add some books", "open one" — and a bare empty array can't tell them
    /// apart.
    enum State: String, Codable, Sendable {
        /// No account on the device, or the snapshot has never been written.
        case signedOut
        /// Signed in, but the library mirror holds nothing.
        case emptyLibrary
        /// Books, but none in progress.
        case nothingInProgress
        /// `books` is non-empty.
        case ready
    }

    /// The most a `systemLarge` card shows. Bounds both the file and the
    /// pre-rendered art beside it.
    static let maxBooks = 5

    var state: State
    var books: [WidgetBook] = []
    var generatedAt: Date

    /// What a widget draws before the app has ever written a snapshot — a
    /// fresh install, or a container the extension cannot reach.
    static func empty(_ state: State = .signedOut) -> WidgetSnapshot {
        WidgetSnapshot(state: state, generatedAt: .distantPast)
    }

    /// The most the medium card's control will flip through.
    ///
    /// Five is what the large family *lists*, and listing is cheap. Flipping is
    /// not: every book past the front costs a tap and a redraw to reach, and
    /// past three the control stops meaning "the other book I'm in" and starts
    /// meaning "page my library" — which is the app's job, not a widget's.
    static let maxFlipped = 3

    /// The books that control walks. The front of the rail, capped.
    var flipRail: [WidgetBook] {
        Array(books.prefix(Self.maxFlipped))
    }

    /// Which book the medium card is showing, and where it sits in that rail.
    ///
    /// The cursor names a *book*, never an index. The rail is rewritten on
    /// every position save, so an index means "whatever is second today" — the
    /// next save that reorders it would move the card out from under the
    /// reader. Naming the book lets it hold its place, and gives an honest
    /// answer once it drops off: it is gone, so the front book takes over.
    /// Same rule as the app's `ContinueHero.selected`.
    func showing(cursor: String?) -> (book: WidgetBook, index: Int)? {
        let rail = flipRail
        guard let first = rail.first else { return nil }
        guard let cursor, let index = rail.firstIndex(where: { $0.id == cursor }) else {
            return (first, 0)
        }
        return (rail[index], index)
    }

    /// The book after the cursor's, wrapping at the end — where the card's
    /// advance control moves to. It wraps because that control is the only way
    /// through the rail: dead-ending on the last book would leave no way back
    /// to the one the reader is actually in the middle of.
    func next(after cursor: String?) -> WidgetBook? {
        let rail = flipRail
        guard let (_, index) = showing(cursor: cursor) else { return nil }
        return rail[(index + 1) % rail.count]
    }
}
