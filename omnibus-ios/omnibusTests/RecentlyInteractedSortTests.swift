//  RecentlyInteractedSortTests.swift
//  The "Recently interacted" sort axis on the native client: its place in the
//  shared wire vocabulary and the picker, the `Book` field it reads, and the
//  ordering the offline mirror produces for it — including where a book nobody
//  has ever touched lands.

import Foundation
import SQLite3
import Testing

@testable import omnibus

// MARK: - Wire vocabulary + picker

struct RecentlyInteractedSortKeyTests {
    @Test func sortKeyUsesTheSharedWireToken() {
        #expect(SortKey.recentlyInteracted.rawValue == "recently_interacted")
        #expect(SortKey(rawValue: "recently_interacted") == .recentlyInteracted)
    }

    @Test func sortPickerOffersRecentlyInteracted() {
        // `allCases` *is* the picker's content, so its membership and order
        // are the control's contract.
        #expect(SortKey.allCases.contains(.recentlyInteracted))
        #expect(SortKey.recentlyInteracted.label == "Recently interacted")
        #expect(
            SortKey.allCases.map(\.rawValue) == [
                "title", "author", "series", "recently_interacted",
                "last_updated", "newest_added",
            ]
        )
    }
}

// MARK: - Decode

struct LastInteractedDecodeTests {
    private func decode(_ json: String) throws -> Book {
        try JSONDecoder().decode(Book.self, from: Data(json.utf8))
    }

    @Test func bookDecodesLastInteractedAt() throws {
        let book = try decode(
            #"{"id":1,"filename":"a.epub","last_interacted_at":"2026-08-29T10:00:00Z"}"#
        )
        #expect(book.lastInteractedAt == "2026-08-29T10:00:00Z")
    }

    @Test func bookDecodesWithoutLastInteractedAt() throws {
        // Listing projections for a never-touched book omit the field, and a
        // payload cached before this field existed has no key at all.
        let book = try decode(#"{"id":1,"filename":"a.epub"}"#)
        #expect(book.lastInteractedAt == nil)
    }
}

// MARK: - Mirror row

struct LastInteractedRowTests {
    private func row(lastInteractedAt: String?) -> OfflineStore.BookRow {
        var book = Book(id: 1, filename: "a.epub", title: "A", uniqueIdentifier: "u1")
        book.lastInteractedAt = lastInteractedAt
        return LibraryIndex.row(for: book, payload: Data())
    }

    @Test func rowCarriesTheInteractionSignal() {
        #expect(row(lastInteractedAt: "2026-08-29T10:00:00Z").lastInteracted
            == "2026-08-29T10:00:00Z")
    }

    @Test func rowLeavesTheSignalEmptyWhenTheBookWasNeverTouched() {
        // Empty rather than absent: the column is NOT NULL, and an empty
        // string is what sorts the book last under the axis's default — where
        // the server's own NULL lands. The signal is library-wide, so this is
        // "nobody has touched it", not "this reader hasn't".
        #expect(row(lastInteractedAt: nil).lastInteracted == "")
    }
}

// MARK: - Ordering, run against a scratch SQLite table

/// Order `(uuid, last_interacted, title)` triples through the exact fragment
/// `LibraryIndex.order` generates, returning the uuids in result order.
private func ordered(
    _ rows: [(uuid: String, lastInteracted: String, title: String)],
    direction: SortDirection
) -> [String] {
    // Every SQLite step is guarded rather than `#expect`ed: a failed `#expect`
    // reports and keeps going, and the next call would then run against a nil
    // handle and take the whole test process down with it. Recording an issue
    // and returning empty fails the caller's assertion with a readable signal.
    var db: OpaquePointer?
    guard sqlite3_open(":memory:", &db) == SQLITE_OK, db != nil else {
        Issue.record("could not open an in-memory database")
        return []
    }
    defer { sqlite3_close(db) }
    guard
        sqlite3_exec(
            db,
            """
            CREATE TABLE books (
                uuid TEXT NOT NULL,
                last_interacted TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT ''
            )
            """,
            nil, nil, nil
        ) == SQLITE_OK
    else {
        Issue.record("could not create the scratch books table")
        return []
    }

    let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
    for row in rows {
        var insert: OpaquePointer?
        guard
            sqlite3_prepare_v2(
                db, "INSERT INTO books (uuid, last_interacted, title) VALUES (?, ?, ?)",
                -1, &insert, nil
            ) == SQLITE_OK
        else {
            Issue.record("could not prepare the insert for \(row.uuid)")
            return []
        }
        defer { sqlite3_finalize(insert) }
        sqlite3_bind_text(insert, 1, row.uuid, -1, transient)
        sqlite3_bind_text(insert, 2, row.lastInteracted, -1, transient)
        sqlite3_bind_text(insert, 3, row.title, -1, transient)
        guard sqlite3_step(insert) == SQLITE_DONE else {
            Issue.record("could not insert \(row.uuid)")
            return []
        }
    }

    let clause = LibraryIndex.order(sort: .recentlyInteracted, direction: direction)
    var stmt: OpaquePointer?
    guard
        sqlite3_prepare_v2(db, "SELECT uuid FROM books ORDER BY \(clause)", -1, &stmt, nil)
            == SQLITE_OK
    else {
        Issue.record("could not prepare a select ordered by: \(clause)")
        return []
    }
    defer { sqlite3_finalize(stmt) }
    var out: [String] = []
    while sqlite3_step(stmt) == SQLITE_ROW {
        out.append(String(cString: sqlite3_column_text(stmt, 0)))
    }
    return out
}

struct RecentlyInteractedOrderTests {
    private let rows: [(uuid: String, lastInteracted: String, title: String)] = [
        ("untouched", "", "b"),
        ("old", "2026-01-01T00:00:00Z", "c"),
        ("newest", "2026-08-29T10:00:00Z", "a"),
    ]

    @Test func descendingPutsTheLatestSignalFirstAndTheUntouchedBookLast() {
        // Descending is the axis's natural direction, and it is what makes an
        // empty signal read as "untouched" rather than "touched first".
        #expect(ordered(rows, direction: .desc) == ["newest", "old", "untouched"])
    }

    @Test func ascendingReversesTheAxisWithoutFallingBackToTheDateColumns() {
        #expect(ordered(rows, direction: .asc) == ["untouched", "old", "newest"])
    }

    @Test func titleBreaksTiesWithinOneInstant() {
        let tied: [(uuid: String, lastInteracted: String, title: String)] = [
            ("zeta", "2026-08-29T10:00:00Z", "z"),
            ("alpha", "2026-08-29T10:00:00Z", "a"),
        ]
        #expect(ordered(tied, direction: .desc) == ["alpha", "zeta"])
    }
}
