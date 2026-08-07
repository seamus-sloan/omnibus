//  HiddenFormatsTests.swift
//  The hidden-formats preference: the cache-key rule (a pref change must miss
//  the cached first page), the mirror's SQL visibility rule (dual-format and
//  physical-only books survive), the row-writer dedupe that rule relies on,
//  and the lenient decode that keeps pre-upgrade `/me` blobs loading.

import Foundation
import SQLite3
import Testing

@testable import omnibus

private func book(
    uuid: String = "book-1", title: String = "Fixture", formats: [String] = ["epub"]
) -> Book {
    Book(id: 1, filename: "\(uuid).epub", title: title, uniqueIdentifier: uuid, formats: formats)
}

// MARK: - Cache key

struct HiddenFormatsSignatureTests {
    @Test func pageSignatureDiffersWhenHiddenFormatsDiffer() {
        var hiding = LibraryFilter()
        hiding.hiddenFormats = ["cbz"]
        let plain = LibraryService.pageSignature(sort: .title, direction: .asc, filter: .none)
        let hidden = LibraryService.pageSignature(sort: .title, direction: .asc, filter: hiding)
        #expect(plain != hidden, "a pref toggle must miss the cached first page")
    }

    @Test func pageSignatureIsOrderInsensitiveOverHiddenFormats() {
        var ab = LibraryFilter()
        ab.hiddenFormats = ["cbz", "m4b"]
        var ba = LibraryFilter()
        ba.hiddenFormats = ["m4b", "cbz"]
        #expect(
            LibraryService.pageSignature(sort: .title, direction: .asc, filter: ab)
                == LibraryService.pageSignature(sort: .title, direction: .asc, filter: ba)
        )
    }
}

// MARK: - Mirror predicate, run against a scratch SQLite table

/// Execute the mirror's visibility predicate against an in-memory `books`
/// table holding one row per formats-CSV, returning the visible CSVs. Runs
/// the exact fragment `LibraryIndex.predicate` generates, so these tests pin
/// SQL semantics, not string shape.
private func visibleRows(formatsCSVs: [String], hiding: [String]) -> Set<String> {
    var db: OpaquePointer?
    #expect(sqlite3_open(":memory:", &db) == SQLITE_OK)
    defer { sqlite3_close(db) }
    sqlite3_exec(db, "CREATE TABLE books (formats TEXT NOT NULL)", nil, nil, nil)
    for csv in formatsCSVs {
        var insert: OpaquePointer?
        sqlite3_prepare_v2(db, "INSERT INTO books (formats) VALUES (?)", -1, &insert, nil)
        sqlite3_bind_text(insert, 1, csv, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
        sqlite3_step(insert)
        sqlite3_finalize(insert)
    }

    var filter = LibraryFilter()
    filter.hiddenFormats = hiding
    let (clause, bindings) = LibraryIndex.predicate(for: filter)
    let sql = "SELECT formats FROM books" + (clause.isEmpty ? "" : " WHERE \(clause)")
    var stmt: OpaquePointer?
    #expect(sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK)
    defer { sqlite3_finalize(stmt) }
    for (i, binding) in bindings.enumerated() {
        sqlite3_bind_text(
            stmt, Int32(i + 1), binding, -1,
            unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        )
    }
    var out: Set<String> = []
    while sqlite3_step(stmt) == SQLITE_ROW {
        out.insert(String(cString: sqlite3_column_text(stmt, 0)))
    }
    return out
}

struct HiddenFormatsPredicateTests {
    @Test func hiddenPredicateKeepsBookWithAnyVisibleFormat() {
        let visible = visibleRows(
            formatsCSVs: ["cbz", "cbz,epub", "epub"], hiding: ["cbz"]
        )
        #expect(visible == ["cbz,epub", "epub"], "the epub side keeps the dual-format book")
    }

    @Test func hiddenPredicateKeepsFormatlessPhysicalOnlyRow() {
        let visible = visibleRows(formatsCSVs: [""], hiding: ["cbz", "epub"])
        #expect(visible == [""], "physical-only rows are never hidden")
    }

    @Test func hiddenPredicateHidesRowWhoseEveryFormatIsHidden() {
        let visible = visibleRows(
            formatsCSVs: ["cbz", "cbz,m4b"], hiding: ["cbz", "m4b"]
        )
        #expect(visible.isEmpty)
    }

    @Test func emptyHidingListLeavesEveryRowVisible() {
        let visible = visibleRows(formatsCSVs: ["cbz", "", "epub"], hiding: [])
        #expect(visible.count == 3)
    }
}

// MARK: - Row writer

struct HiddenFormatsRowTests {
    @Test func rowForBookDedupesDuplicateFormats() {
        // The predicate strips each hidden token once, so a duplicate in the
        // CSV would survive the strip and read as "still visible".
        let b = book(formats: ["CBZ", "cbz", "EPUB"])
        let row = LibraryIndex.row(for: b, payload: Data())
        #expect(row.formats == "cbz,epub")
    }

    @Test func rowForPhysicallyOwnedBookCarriesThePhysicalToken() {
        var b = book(formats: ["CBZ"])
        b.hasPhysical = true
        let row = LibraryIndex.row(for: b, payload: Data())
        #expect(row.formats == "cbz,physical")
    }
}

extension HiddenFormatsPredicateTests {
    @Test func hiddenPredicateKeepsPhysicallyOwnedBookWithAllFormatsHidden() {
        // The physical pseudo-token survives the strip, mirroring the
        // server's physical arm.
        let visible = visibleRows(formatsCSVs: ["cbz,physical", "cbz"], hiding: ["cbz"])
        #expect(visible == ["cbz,physical"])
    }
}

// MARK: - Wire decode

struct HiddenFormatsDecodeTests {
    @Test func userSummaryDecodeDefaultsHiddenFormatsWhenAbsent() throws {
        // A pre-upgrade cached `/me` blob has no hidden_formats key.
        let json = """
            {"id":1,"username":"alice","is_admin":false,
             "can_upload":false,"can_edit":false,"can_download":true}
            """
        let me = try JSONDecoder().decode(UserSummary.self, from: Data(json.utf8))
        #expect(me.hiddenFormats.isEmpty)
    }
}
