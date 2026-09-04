//  LibrarySortPersistenceTests.swift
//  The library sort survives a relaunch (#2347): the preference round-trips
//  through `UserDefaults`, tolerates a raw value the build no longer knows,
//  and `LibraryModel` restores it at init and writes it back on change.

import Foundation
import Testing

@testable import omnibus

/// A throwaway suite so the tests never touch the host's real defaults.
private func scratchDefaults() -> UserDefaults {
    let name = "omnibus.tests.librarySort.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: name)!
    defaults.removePersistentDomain(forName: name)
    return defaults
}

struct LibrarySortPreferenceTests {
    @Test func loadReturnsTheFactoryDefaultsWhenNothingIsStored() {
        let pref = LibrarySortPreference.load(from: scratchDefaults())
        #expect(pref == LibrarySortPreference(sort: .newestAdded, direction: .desc))
    }

    @Test func saveThenLoadRoundTripsANonDefaultChoice() {
        let defaults = scratchDefaults()
        LibrarySortPreference(sort: .author, direction: .asc).save(to: defaults)
        let pref = LibrarySortPreference.load(from: defaults)
        #expect(pref == LibrarySortPreference(sort: .author, direction: .asc))
    }

    @Test func loadFallsBackPerFieldWhenARawValueIsUnknown() {
        let defaults = scratchDefaults()
        defaults.set("no_such_sort", forKey: LibrarySortPreference.sortKey)
        defaults.set(SortDirection.asc.rawValue, forKey: LibrarySortPreference.directionKey)
        let pref = LibrarySortPreference.load(from: defaults)
        #expect(pref.sort == .newestAdded, "an unknown sort key costs that one choice")
        #expect(pref.direction == .asc, "the direction beside it is kept")
    }
}

@MainActor
struct LibraryModelSortPersistenceTests {
    @Test func modelRestoresTheStoredSortAtInit() {
        let defaults = scratchDefaults()
        LibrarySortPreference(sort: .series, direction: .asc).save(to: defaults)
        let model = LibraryModel(defaults: defaults)
        #expect(model.sort == .series)
        #expect(model.direction == .asc)
    }

    @Test func changingTheSortWritesItThrough() {
        let defaults = scratchDefaults()
        let model = LibraryModel(defaults: defaults)
        model.sort = .title
        model.direction = .asc
        let pref = LibrarySortPreference.load(from: defaults)
        #expect(pref == LibrarySortPreference(sort: .title, direction: .asc))
    }
}
