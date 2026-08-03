//  HighlightsListTests.swift
//  How many saved passages the book-detail list renders before the reader
//  expands it.
//
//  The section used to hard-cap at six with no way to see the rest, so a
//  heavily highlighted book silently hid everything past the sixth passage.

import Testing

@testable import omnibus

@Test func collapsedListRendersAtMostTheCollapsedCount() {
    #expect(HighlightsList.visibleCount(total: 40, expanded: false) == HighlightsList.collapsedCount)
}

@Test func collapsedListRendersEveryPassageWhenItFitsUnderTheCap() {
    #expect(HighlightsList.visibleCount(total: 3, expanded: false) == 3)
    #expect(
        HighlightsList.visibleCount(total: HighlightsList.collapsedCount, expanded: false)
            == HighlightsList.collapsedCount
    )
}

@Test func expandedListRendersEveryPassage() {
    #expect(HighlightsList.visibleCount(total: 40, expanded: true) == 40)
}

@Test func emptyListRendersNothingInEitherState() {
    #expect(HighlightsList.visibleCount(total: 0, expanded: false) == 0)
    #expect(HighlightsList.visibleCount(total: 0, expanded: true) == 0)
}
