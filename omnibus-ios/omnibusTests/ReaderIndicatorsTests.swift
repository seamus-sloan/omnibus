//  ReaderIndicatorsTests.swift
//  What the reader's two centred labels say, per chrome state.
//
//  Pinned here because the wording is the whole feature: the same two lines
//  serve reading (title, page) and navigating (pages left, page of total), and
//  each has its own fallback for the seconds before epub.js's locations pass
//  lands. Rendered, all of that is one 12.5pt line that is easy to eyeball as
//  fine while showing the wrong half of it.

import Foundation
import Testing

@testable import omnibus

/// A relocate with whole-book page numbers — what the reader reports once
/// epub.js's locations pass has run.
private func located(
    page: Int = 361, totalPages: Int = 661, pct: Int = 55,
    chapter: Int = 7, totalChapters: Int = 24, chapterPagesLeft: Int = 12
) -> RelocateData {
    RelocateData(
        cfi: "epubcfi(/6/14!/4/2/2)", page: page, totalPages: totalPages, pct: pct,
        chapter: chapter, totalChapters: totalChapters, chapterTitle: "The Moor",
        chapterPagesLeft: chapterPagesLeft
    )
}

/// The first seconds of a book: a position is known, page numbers are not.
private func unlocated(pct: Int = 3) -> RelocateData {
    RelocateData(
        cfi: "epubcfi(/6/2!/4/2)", page: 0, totalPages: 0, pct: pct,
        chapter: 1, totalChapters: 24, chapterTitle: "Mr. Sherlock Holmes",
        chapterPagesLeft: 0
    )
}

private func indicators(
    _ location: RelocateData?, title: String = "The Hound of the Baskervilles",
    chromeVisible: Bool
) -> ReaderIndicators {
    ReaderIndicators(location: location, title: title, chromeVisible: chromeVisible)
}

@Suite("Reader indicators while reading")
struct ReaderIndicatorsHiddenChromeTests {
    @Test("the top line is the book's title when the chrome is down")
    func topIsTheTitle() {
        #expect(indicators(located(), chromeVisible: false).top == "The Hound of the Baskervilles")
    }

    @Test("the bottom line is the bare page number when the chrome is down")
    func bottomIsTheBarePage() {
        #expect(indicators(located(), chromeVisible: false).bottom == "361")
    }

    /// The title is the only line that has nothing to fall back to, so a book
    /// filed under a blank title has to leave the band empty rather than paint
    /// an empty label.
    @Test("a blank title leaves the top line empty rather than showing whitespace")
    func blankTitleShowsNothing() {
        #expect(indicators(located(), title: "  ", chromeVisible: false).top == nil)
    }

    /// The page number is still 0 in the seconds before the locations pass
    /// lands, so "0" would be both wrong and jumpy — percent is the honest
    /// stand-in, chrome or no chrome.
    @Test("the bottom line falls back to percent before page numbers exist")
    func bottomFallsBackToPercent() {
        #expect(indicators(unlocated(pct: 3), chromeVisible: false).bottom == "3%")
    }

    @Test("nothing shows before the first relocate, so both labels arrive together")
    func nothingBeforeTheFirstRelocate() {
        let labels = indicators(nil, chromeVisible: false)
        #expect(labels.top == nil)
        #expect(labels.bottom == nil)
    }
}

@Suite("Reader indicators with the menu up")
struct ReaderIndicatorsVisibleChromeTests {
    @Test("the top line counts the pages left in the chapter")
    func topCountsPagesLeft() {
        #expect(
            indicators(located(chapterPagesLeft: 12), chromeVisible: true).top
                == "12 pages left in chapter"
        )
    }

    @Test("the last page of a chapter reads in the singular")
    func onePageLeftIsSingular() {
        #expect(
            indicators(located(chapterPagesLeft: 1), chromeVisible: true).top
                == "1 page left in chapter"
        )
    }

    @Test("the top line falls back to the chapter's position when no pages are left to count")
    func topFallsBackToChapterPosition() {
        #expect(
            indicators(located(chapterPagesLeft: 0), chromeVisible: true).top
                == "Chapter 7 of 24"
        )
    }

    /// Front matter routinely matches no TOC entry, which lands here as chapter
    /// 0 — a "Chapter 0 of 24" nobody can act on.
    @Test("the top line is empty in front matter, which belongs to no chapter")
    func topIsEmptyInFrontMatter() {
        #expect(
            indicators(located(chapter: 0, chapterPagesLeft: 0), chromeVisible: true).top == nil
        )
    }

    @Test("the bottom line carries the page count as well as the page")
    func bottomCarriesTheTotal() {
        #expect(indicators(located(), chromeVisible: true).bottom == "361 of 661")
    }

    @Test("the bottom line falls back to percent before page numbers exist")
    func bottomFallsBackToPercent() {
        #expect(indicators(unlocated(pct: 3), chromeVisible: true).bottom == "3%")
    }

    /// Percent is 0 for the whole first page of a book, and "0%" reads as a
    /// stall rather than a position.
    @Test("the bottom line is empty when there is neither a page nor a percent")
    func bottomIsEmptyAtZero() {
        #expect(indicators(unlocated(pct: 0), chromeVisible: true).bottom == nil)
    }
}
