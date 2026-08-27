//  ContinueWidget.swift
//  The Home Screen equivalent of the landing Continue stack.
//
//  Reads nothing but the App Group snapshot, so it renders identically in
//  Airplane Mode with the app force-quit — which is most of the time a widget
//  is actually looked at.

import AppIntents
import SwiftUI
import WidgetKit

struct ContinueEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot
    /// Which book this copy of the widget is pinned to, by `WidgetBook.id`.
    /// Resolved at timeline time rather than inside the view, so a render stays
    /// a pure function of its entry and a `#Preview` can pose any book.
    var selected: String?
}

struct ContinueProvider: AppIntentTimelineProvider {
    /// How long a timeline stands before WidgetKit asks for another.
    ///
    /// The app pushes a reload whenever the snapshot changes, so this is only
    /// the backstop for the case it can't: the app was removed from memory
    /// before it could reload, or the reload was coalesced away. An hour,
    /// because the only thing on the card that decays is the "last read" line,
    /// and it is formatted in whole hours and days — an hour late on "3d ago"
    /// is invisible, and WidgetKit's refresh budget is not worth spending to
    /// be exact about it.
    private static let refreshInterval: TimeInterval = 3600

    func placeholder(in context: Context) -> ContinueEntry {
        ContinueEntry(date: .now, snapshot: .preview)
    }

    func snapshot(for configuration: SelectBookIntent, in context: Context) async -> ContinueEntry {
        // The gallery preview has no App Group content to show yet, so it gets
        // the same fabricated card the placeholder does — a real-looking
        // widget is what someone is choosing between in that list.
        let snapshot = context.isPreview ? .preview : (WidgetStore.load() ?? .empty())
        return ContinueEntry(
            date: .now,
            snapshot: snapshot,
            selected: context.isPreview ? nil : configuration.book?.id
        )
    }

    func timeline(for configuration: SelectBookIntent, in context: Context) async -> Timeline<ContinueEntry> {
        let entry = ContinueEntry(
            date: .now,
            snapshot: WidgetStore.load() ?? .empty(),
            selected: configuration.book?.id
        )
        return Timeline(entries: [entry], policy: .after(.now + Self.refreshInterval))
    }

    /// One configuration per book in progress, so a Smart Stack has something
    /// to rotate between rather than one card that never changes.
    ///
    /// Grouped rather than loose: these are alternative views of one widget,
    /// not five widgets someone asked for, and an ungrouped set can stack up as
    /// five separate faces of the same thing.
    func relevance() async -> WidgetRelevance<SelectBookIntent> {
        let books = (WidgetStore.load() ?? .empty()).books.prefix(WidgetSnapshot.maxBooks)
        return WidgetRelevance(
            books.map {
                WidgetRelevanceAttribute(
                    configuration: SelectBookIntent(book: BookEntity($0)),
                    group: .named(WidgetKind.continueReading)
                )
            }
        )
    }

    /// What the widget gallery offers when the reader is adding one. The
    /// unpinned card leads: it is the one that keeps working without being
    /// revisited, and the pinned ones are what a stack is built out of
    /// afterwards.
    func recommendations() -> [AppIntentRecommendation<SelectBookIntent>] {
        let books = (WidgetStore.load() ?? .empty()).books.prefix(WidgetSnapshot.maxBooks)
        return [AppIntentRecommendation(intent: SelectBookIntent(), description: Text("Most recent"))]
            + books.map {
                AppIntentRecommendation(
                    intent: SelectBookIntent(book: BookEntity($0)),
                    description: Text($0.title)
                )
            }
    }
}

struct ContinueWidget: Widget {
    var body: some WidgetConfiguration {
        AppIntentConfiguration(
            kind: WidgetKind.continueReading,
            intent: SelectBookIntent.self,
            provider: ContinueProvider()
        ) { entry in
            ContinueWidgetView(snapshot: entry.snapshot, selected: entry.selected)
        }
        .configurationDisplayName("Continue")
        // Says the stack out loud: the card shows one book, and the way to see
        // the others is to add a second copy and stack them.
        .description("Pick up a book you're in the middle of. Add one per book and stack them to swipe between.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
        // The card is a full-bleed wash in the book's own colour with cover
        // art laid on it, so it owns its insets: the system's default margins
        // would leave a border of Home Screen showing inside the ground.
        .contentMarginsDisabled()
    }
}

extension WidgetSnapshot {
    /// A fabricated card for the widget gallery and the placeholder, where
    /// there is no snapshot yet and an empty state would misrepresent what the
    /// widget does.
    ///
    /// The timestamps are relative to now rather than fixed: the large family
    /// renders "last read" in relative style, and a constant date would read as
    /// "55 years ago" in the gallery someone is choosing the widget from.
    static let preview = WidgetSnapshot(
        state: .ready,
        books: [
            WidgetBook(
                bookUUID: "preview-1",
                format: .epub,
                title: "The Voyage of the Beagle",
                author: "Charles Darwin",
                tone: WidgetBook.Tone(l: 0.55, c: 0.11, h: 42),
                fraction: 0.38,
                updatedAt: .now.addingTimeInterval(-2 * 3600)
            ),
            WidgetBook(
                bookUUID: "preview-2",
                format: .audio,
                title: "The Sea, the Sea",
                author: "Iris Murdoch",
                tone: WidgetBook.Tone(l: 0.55, c: 0.10, h: 236),
                fraction: 0.61,
                secondsRemaining: 4 * 3600 + 12 * 60,
                updatedAt: .now.addingTimeInterval(-26 * 3600)
            ),
            WidgetBook(
                bookUUID: "preview-3",
                format: .epub,
                title: "A Room with a View",
                author: "E. M. Forster",
                tone: WidgetBook.Tone(l: 0.55, c: 0.09, h: 150),
                fraction: 0.12,
                updatedAt: .now.addingTimeInterval(-4 * 86400)
            ),
        ],
        generatedAt: .now
    )
}

// Every family and every empty state, without adding the widget to a Home
// Screen and reading a book to populate it — which is otherwise the only way
// to see four of these six.
#Preview("Small", as: .systemSmall) {
    ContinueWidget()
} timeline: {
    ContinueEntry(date: .distantPast, snapshot: .preview)
    ContinueEntry(date: .distantPast, snapshot: .empty(.nothingInProgress))
}

#Preview("Medium", as: .systemMedium) {
    ContinueWidget()
} timeline: {
    ContinueEntry(date: .distantPast, snapshot: .preview)
    ContinueEntry(date: .distantPast, snapshot: .empty(.emptyLibrary))
}

#Preview("Large", as: .systemLarge) {
    ContinueWidget()
} timeline: {
    ContinueEntry(date: .distantPast, snapshot: .preview)
    ContinueEntry(date: .distantPast, snapshot: .empty(.signedOut))
}
