//  MainTabChromeTests.swift
//  What decides whether the bottom bar is up: which destinations hold the
//  bottom edge, whether a stack is holding it, and that each tab reads the
//  stack it owns.

import Foundation
import Testing

@testable import omnibus

// MARK: - Which screens hold the bottom edge

@Test func onlyTheBookDetailYieldsTheBottomEdge() {
    #expect(Destination.book(uuid: "u").hidesTabBar)
    #expect(!Destination.metadataEdit(uuid: "u").hidesTabBar)
    #expect(!Destination.author(id: 1).hidesTabBar)
    #expect(!Destination.shelves.hidesTabBar)
    #expect(!Destination.settings.hidesTabBar)
}

@Test func aStackYieldsTheBottomEdgeUntilTheDetailItselfPops() {
    #expect(![Destination]().hidesTabBar)
    #expect(![Destination.shelves, .shelf(id: 1)].hidesTabBar)

    // A detail pushed over a detail, and the metadata editor pushed over
    // one: both still hold the edge, so the answer is about the whole stack
    // rather than its top.
    #expect([Destination.book(uuid: "a"), .book(uuid: "b")].hidesTabBar)
    #expect([Destination.book(uuid: "a"), .metadataEdit(uuid: "a")].hidesTabBar)

    // And popping the last detail is what brings the bar back.
    #expect(![Destination.author(id: 1)].hidesTabBar)
}

// MARK: - Each tab reads its own stack

@Test func tabPathsSubscriptReadsEachTabsOwnStack() {
    var paths = TabPaths()
    paths.library = [.book(uuid: "library")]
    paths.search = [.author(id: 1)]
    paths.stats = [.readingGoals]
    paths.you = [.settings]

    // A transposed case in the subscript type checks perfectly and would
    // simply answer for the wrong tab, so each pairing is pinned by hand.
    #expect(paths[.library] == [.book(uuid: "library")])
    #expect(paths[.search] == [.author(id: 1)])
    #expect(paths[.stats] == [.readingGoals])
    #expect(paths[.you] == [.settings])
}

@Test func onlyTheSelectedTabsStackDecidesTheBar() {
    var paths = TabPaths()
    paths.library = [.book(uuid: "a")]

    // Parking one tab on a detail must not take the bar away from the others
    // — that is the whole reason the answer is scoped to the selection.
    #expect(paths[.library].hidesTabBar)
    #expect(!paths[.search].hidesTabBar)
    #expect(!paths[.stats].hidesTabBar)
    #expect(!paths[.you].hidesTabBar)
}

@Test func aFreshTabPathsHoldsNothing() {
    let paths = TabPaths()
    for tab in AppTab.allCases {
        #expect(paths[tab].isEmpty)
        #expect(!paths[tab].hidesTabBar)
    }
}
