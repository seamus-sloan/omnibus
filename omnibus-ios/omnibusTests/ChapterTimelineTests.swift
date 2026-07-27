//  ChapterTimelineTests.swift
//  The chapter geometry the player's chapter-scoped scrubber is built on.
//
//  Pure arithmetic over a chapter list, which is the whole reason it's pinned
//  here: the scrubber, the "Ch N of M" readout, the chapter-remaining countdown
//  and the prev/next enablement all read the same three functions, so an
//  off-by-one shows up in four places at once and in none of them obviously.

import Foundation
import Testing

@testable import omnibus

/// A chapter with an explicit length, the shape a well-tagged M4B gives.
private func chapter(_ ordinal: Int64, start: Double, length: Double) -> ChapterInfo {
    ChapterInfo(ordinal: ordinal, title: "Part \(ordinal)", startSeconds: start, durationSeconds: length)
}

/// A chapter that ships no length of its own — common enough in real files that
/// the fallback is load-bearing rather than defensive.
private func untimed(_ ordinal: Int64, start: Double) -> ChapterInfo {
    ChapterInfo(ordinal: ordinal, title: "Part \(ordinal)", startSeconds: start, durationSeconds: 0)
}

@Suite("Chapter timeline")
struct ChapterTimelineTests {
    private let timeline = ChapterTimeline(
        chapters: [
            chapter(1, start: 0, length: 100),
            chapter(2, start: 100, length: 200),
            chapter(3, start: 300, length: 150),
        ],
        bookDuration: 450
    )

    // MARK: - Which chapter

    @Test("index resolves the chapter a position falls inside")
    func indexResolvesEnclosingChapter() {
        #expect(timeline.index(at: 0) == 0)
        #expect(timeline.index(at: 99) == 0)
        #expect(timeline.index(at: 150) == 1)
        #expect(timeline.index(at: 449) == 2)
    }

    @Test("a position exactly on a boundary belongs to the chapter it opens")
    func boundaryBelongsToTheChapterItOpens() {
        #expect(timeline.index(at: 100) == 1)
        #expect(timeline.index(at: 300) == 2)
    }

    @Test("a position past the end stays in the last chapter")
    func pastTheEndStaysInTheLastChapter() {
        #expect(timeline.index(at: 10_000) == 2)
    }

    /// The case the old lookup returned `nil` for, which left the chapter bar
    /// blank and both chapter buttons dead for the opening seconds of any book
    /// whose first mark isn't at zero.
    @Test("a position before the first chapter's start resolves to that chapter")
    func beforeTheFirstMarkResolvesToTheFirstChapter() {
        let late = ChapterTimeline(
            chapters: [chapter(1, start: 5, length: 95), chapter(2, start: 100, length: 100)],
            bookDuration: 200
        )
        #expect(late.index(at: 0) == 0)
        #expect(late.chapter(at: 0)?.ordinal == 1)
    }

    @Test("an empty timeline has no chapter at any position")
    func emptyTimelineHasNoChapter() {
        let empty = ChapterTimeline(bookDuration: 500)
        #expect(empty.isEmpty)
        #expect(empty.index(at: 0) == nil)
        #expect(empty.chapter(at: 250) == nil)
    }

    // MARK: - Ordering

    @Test("chapters are sorted on the way in, whatever order the manifest gave")
    func chaptersAreSortedOnInit() {
        let shuffled = ChapterTimeline(
            chapters: [
                chapter(3, start: 300, length: 150),
                chapter(1, start: 0, length: 100),
                chapter(2, start: 100, length: 200),
            ],
            bookDuration: 450
        )
        #expect(shuffled.chapters.map(\.ordinal) == [1, 2, 3])
        #expect(shuffled.index(at: 150) == 1)
    }

    // MARK: - How long

    @Test("length uses the chapter's own duration when it has one")
    func lengthUsesDeclaredDuration() {
        #expect(timeline.length(at: 0) == 100)
        #expect(timeline.length(at: 1) == 200)
    }

    @Test("length falls back to the next chapter's start when the file gives none")
    func lengthFallsBackToNextStart() {
        let untimedTimeline = ChapterTimeline(
            chapters: [untimed(1, start: 0), untimed(2, start: 120), untimed(3, start: 400)],
            bookDuration: 600
        )
        #expect(untimedTimeline.length(at: 0) == 120)
        #expect(untimedTimeline.length(at: 1) == 280)
    }

    @Test("the last untimed chapter measures to the end of the book")
    func lastUntimedChapterMeasuresToBookEnd() {
        let untimedTimeline = ChapterTimeline(
            chapters: [untimed(1, start: 0), untimed(2, start: 400)],
            bookDuration: 600
        )
        #expect(untimedTimeline.length(at: 1) == 200)
    }

    @Test("length is zero rather than negative when the book duration is unknown")
    func lengthNeverGoesNegative() {
        let noDuration = ChapterTimeline(chapters: [untimed(1, start: 90)], bookDuration: 0)
        #expect(noDuration.length(at: 0) == 0)
    }

    @Test("length is zero for an index outside the timeline")
    func lengthIsZeroOutOfBounds() {
        #expect(timeline.length(at: -1) == 0)
        #expect(timeline.length(at: 99) == 0)
    }

    // MARK: - The scrubbed span

    @Test("span is the enclosing chapter's start and length")
    func spanIsTheEnclosingChapter() {
        let span = timeline.span(at: 150)
        #expect(span.start == 100)
        #expect(span.duration == 200)
    }

    /// What keeps a book with no chapter marks scrubbing over its whole self
    /// instead of over a zero-length span.
    @Test("span is the whole book when there are no chapters")
    func spanCoversWholeBookWithoutChapters() {
        let empty = ChapterTimeline(bookDuration: 500)
        let span = empty.span(at: 250)
        #expect(span.start == 0)
        #expect(span.duration == 500)
    }

    @Test("an offset inside a span maps back to an absolute position")
    func offsetMapsBackToAbsolutePosition() {
        let span = timeline.span(at: 350)
        #expect(span.start + 20 == 320)
    }
}
