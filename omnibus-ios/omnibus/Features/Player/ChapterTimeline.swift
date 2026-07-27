//  ChapterTimeline.swift
//  The chapter arithmetic behind the player's chapter-scoped scrubber.
//
//  A value type over the sorted chapter list, so "which chapter is this, how
//  long does it run, where does its span start" can be pinned by tests without
//  an AVPlayer behind it. `AudioPlayer` holds one and delegates to it.

import Foundation

struct ChapterTimeline {
    /// Chapters in timeline order.
    let chapters: [ChapterInfo]

    /// Whole-book duration. The last chapter's length falls back to it, and it
    /// is the span reported when there are no chapters at all.
    let bookDuration: Double

    /// Sorts on the way in: the manifest orders by container ordinal, and every
    /// lookup here is positional.
    init(chapters: [ChapterInfo] = [], bookDuration: Double = 0) {
        self.chapters = chapters.sorted { $0.startSeconds < $1.startSeconds }
        self.bookDuration = max(0, bookDuration)
    }

    var isEmpty: Bool { chapters.isEmpty }

    var count: Int { chapters.count }

    /// Index of the chapter `position` falls in.
    ///
    /// `nil` only when there are no chapters. A position *before* the first
    /// chapter's start still resolves to it — some containers put chapter one a
    /// second or two in, and "no chapter" is not a place you can be listening.
    func index(at position: Double) -> Int? {
        guard !chapters.isEmpty else { return nil }
        return chapters.lastIndex { position >= $0.startSeconds } ?? 0
    }

    func chapter(at position: Double) -> ChapterInfo? {
        index(at: position).map { chapters[$0] }
    }

    /// A chapter's length, falling back to the gap up to the next chapter — a
    /// good number of real audiobooks ship `duration_seconds` as 0, and a
    /// zero-length span traps the scrubber at one end.
    func length(at index: Int) -> Double {
        guard chapters.indices.contains(index) else { return 0 }
        let chapter = chapters[index]
        if chapter.durationSeconds > 0 { return chapter.durationSeconds }
        let end = index + 1 < chapters.count ? chapters[index + 1].startSeconds : bookDuration
        return max(0, end - chapter.startSeconds)
    }

    /// Start and length of the span the scrubber covers at `position`: the
    /// current chapter, or the whole book when there is nothing to scope it to.
    func span(at position: Double) -> (start: Double, duration: Double) {
        guard let index = index(at: position) else { return (0, bookDuration) }
        return (chapters[index].startSeconds, length(at: index))
    }
}
