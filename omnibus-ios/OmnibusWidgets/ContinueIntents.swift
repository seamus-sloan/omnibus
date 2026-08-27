//  ContinueIntents.swift
//  What makes one Continue widget different from the next one beside it.
//
//  A widget cannot be swiped — the system renders it as a snapshot and only
//  routes taps — so the rail is walked by *stacking* widgets and swiping the
//  stack. That only shows different books if each copy can be configured to a
//  different one, which is what this is: the book parameter the Home Screen's
//  edit sheet offers, and the entity query behind it.

import AppIntents

/// A book the widget can be pinned to, named by `WidgetBook.id` so the
/// selection survives the rail being rewritten around it.
struct BookEntity: AppEntity {
    let id: String
    let title: String
    let author: String

    init(_ book: WidgetBook) {
        id = book.id
        title = book.title
        author = book.author
    }

    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Book"
    static let defaultQuery = BookQuery()

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(title)", subtitle: "\(author)")
    }
}

/// Answers the configuration sheet out of the App Group snapshot.
///
/// The books in progress are the only ones offered. Pinning a widget to
/// something the reader is not currently reading would be a widget that says
/// "Continue" about a book they never started — and the snapshot is all the
/// extension can see anyway.
struct BookQuery: EntityQuery {
    func entities(for identifiers: [BookEntity.ID]) async throws -> [BookEntity] {
        books.filter { identifiers.contains($0.id) }
    }

    func suggestedEntities() async throws -> [BookEntity] {
        books
    }

    private var books: [BookEntity] {
        (WidgetStore.load() ?? .empty()).books.map(BookEntity.init)
    }
}

struct SelectBookIntent: WidgetConfigurationIntent {
    static let title: LocalizedStringResource = "Choose a book"
    static let description = IntentDescription(
        "Pick the book this widget shows. Left empty it follows whichever book you read last."
    )

    /// Optional on purpose, and the empty case is the default: a widget added
    /// without a thought should track the reader rather than pin to whatever
    /// happened to be on top the day they added it. Pinning is what you do to
    /// the *second* copy, once you are building a stack.
    @Parameter(title: "Book")
    var book: BookEntity?

    init() {}

    init(book: BookEntity?) {
        self.book = book
    }
}
