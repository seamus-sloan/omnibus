//  SuggestionPoolTests.swift
//  The pure half of the autocomplete dropdown: pool collection, query
//  filtering, and the "+ Create" row gate — ported behavior from the web's
//  suggestion_dropdown, so the two clients must keep agreeing.

import Testing

@testable import omnibus

struct SuggestionPoolTests {
    @Test func collectDedupsCaseInsensitivelyAndKeepsHigherCount() {
        let pool = SuggestionPool.collect([
            SuggestionItem(name: "Sarah J. Maas", count: 8),
            SuggestionItem(name: "sarah j. maas", count: 3),
            SuggestionItem(name: "Brandon Sanderson", count: 12),
        ])
        #expect(pool.count == 2)
        let maas = pool.first { $0.name == "Sarah J. Maas" }
        #expect(maas?.count == 8)
        #expect(pool.contains { $0.name == "Brandon Sanderson" && $0.count == 12 })
    }

    @Test func collectSortsAlphabeticallyIgnoringCase() {
        let pool = SuggestionPool.collect([
            SuggestionItem(name: "zelda", count: 0),
            SuggestionItem(name: "Ada", count: 5),
            SuggestionItem(name: "Mira", count: 1),
        ])
        #expect(pool.map(\.name) == ["Ada", "Mira", "zelda"])
    }

    @Test func filteredMatchesSubstringIgnoringCase() {
        let pool = [
            SuggestionItem(name: "Science Fiction", count: 4),
            SuggestionItem(name: "Historical Fiction", count: 2),
            SuggestionItem(name: "Poetry", count: 1),
        ]
        let rows = SuggestionPool.filtered(pool: pool, current: [], query: "fic")
        #expect(rows.map(\.name) == ["Science Fiction", "Historical Fiction"])
    }

    @Test func filteredExcludesValuesAlreadyPresent() {
        let pool = [
            SuggestionItem(name: "Fantasy", count: 6),
            SuggestionItem(name: "Fantasy Romance", count: 2),
        ]
        let rows = SuggestionPool.filtered(pool: pool, current: ["fantasy"], query: "fan")
        #expect(rows.map(\.name) == ["Fantasy Romance"])
    }

    @Test func filteredEmptyQueryReturnsPoolHeadMinusPresent() {
        let pool = (1...8).map { SuggestionItem(name: "Tag \($0)", count: $0) }
        let rows = SuggestionPool.filtered(pool: pool, current: ["Tag 1"], query: "  ")
        #expect(rows.count == SuggestionPool.maxSuggestions)
        #expect(rows.first?.name == "Tag 2")
    }

    @Test func filteredCapsAtFiveMatches() {
        let pool = (1...9).map { SuggestionItem(name: "Series \($0)", count: 1) }
        let rows = SuggestionPool.filtered(pool: pool, current: [], query: "series")
        #expect(rows.count == 5)
    }

    @Test func createRowShowsForNovelTypedText() {
        let pool = [SuggestionItem(name: "Fantasy", count: 6)]
        #expect(SuggestionPool.showsCreateRow(pool: pool, current: [], typed: " Cozy Fantasy "))
    }

    @Test func createRowHiddenWhenPoolHasExactMatchIgnoringCase() {
        let pool = [SuggestionItem(name: "Fantasy", count: 6)]
        #expect(!SuggestionPool.showsCreateRow(pool: pool, current: [], typed: "fantasy"))
    }

    @Test func createRowHiddenWhenValueAlreadyChosenOrBlank() {
        #expect(!SuggestionPool.showsCreateRow(pool: [], current: ["Fantasy"], typed: "FANTASY"))
        #expect(!SuggestionPool.showsCreateRow(pool: [], current: [], typed: "   "))
    }

    @Test func createRowChecksFullPoolNotTruncatedView() {
        // "tag" matches every entry, so the exact match "Tag" falls past the
        // top-five cut — it must still suppress the Create row.
        var pool = (1...6).map { SuggestionItem(name: "Tag \($0)", count: 1) }
        pool.append(SuggestionItem(name: "Tag", count: 1))
        let rows = SuggestionPool.filtered(pool: pool, current: [], query: "tag")
        #expect(!rows.contains { $0.name == "Tag" })
        #expect(!SuggestionPool.showsCreateRow(pool: pool, current: [], typed: "Tag"))
    }
}
