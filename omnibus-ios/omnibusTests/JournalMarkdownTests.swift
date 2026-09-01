//  JournalMarkdownTests.swift
//  The block splitter and spoiler pass behind a journal body: what becomes a
//  paragraph, a list, a heading or a rule, and where a `||spoiler||` starts
//  and stops.

import Foundation
import Testing

@testable import omnibus

/// Most blocks carry one plain run; asserting on that reads better than on the
/// span array.
private func plain(_ spans: [JournalSpan]) -> String {
    spans.map { span in
        switch span {
        case .prose(let text): text
        case .spoiler(let text, _): text
        }
    }.joined()
}

// MARK: - Blocks

@Test func blocksSplitParagraphsOnBlankLines() {
    let blocks = JournalMarkdown.blocks("First thought.\n\nSecond thought.")
    #expect(blocks == [
        .paragraph([.prose("First thought.")]),
        .paragraph([.prose("Second thought.")]),
    ])
}

@Test func blocksKeepAuthoredLineBreaksInsideOneParagraph() {
    // The server promotes a soft break to a hard one, so a newline the author
    // can see has to stay visible rather than collapse to a space.
    let blocks = JournalMarkdown.blocks("One line\nand its wrap")
    #expect(blocks == [.paragraph([.prose("One line\nand its wrap")])])
}

@Test func blocksGatherDashedLinesIntoOneBulletList() {
    let blocks = JournalMarkdown.blocks("- first\n- second\n- third")
    #expect(blocks == [.bullets([
        [.prose("first")], [.prose("second")], [.prose("third")],
    ])])
}

@Test func blocksAcceptEveryBulletMarker() {
    let blocks = JournalMarkdown.blocks("- dash\n* star\n+ plus")
    #expect(blocks == [.bullets([[.prose("dash")], [.prose("star")], [.prose("plus")]])])
}

@Test func blocksNumberAnOrderedList() {
    let blocks = JournalMarkdown.blocks("1. first\n2) second")
    #expect(blocks == [.numbered([[.prose("first")], [.prose("second")]])])
}

@Test func blocksContinueAListItemAcrossALazyWrap() {
    // A plain line under an open list belongs to its last item, not to a new
    // paragraph that would break the list in two.
    let blocks = JournalMarkdown.blocks("- a long thought\nthat wrapped\n- the next one")
    #expect(blocks == [.bullets([
        [.prose("a long thought\nthat wrapped")], [.prose("the next one")],
    ])])
}

@Test func blocksEndAListAtABlankLine() {
    let blocks = JournalMarkdown.blocks("- item\n\nBack to prose.")
    #expect(blocks == [
        .bullets([[.prose("item")]]),
        .paragraph([.prose("Back to prose.")]),
    ])
}

@Test func blocksReadAnATXHeadingAndItsLevel() {
    let blocks = JournalMarkdown.blocks("## The Highlights\nA thought.")
    #expect(blocks == [
        .heading(level: 2, spans: [.prose("The Highlights")]),
        .paragraph([.prose("A thought.")]),
    ])
}

@Test func blocksDropAHeadingsClosingHashes() {
    let blocks = JournalMarkdown.blocks("# Title #")
    #expect(blocks == [.heading(level: 1, spans: [.prose("Title")])])
}

@Test func blocksKeepASharpThatBelongsToTheHeadingsLastWord() {
    // A closing run only closes when a space runs up to it, so the language
    // keeps its name.
    let blocks = JournalMarkdown.blocks("# Notes on C#")
    #expect(blocks == [.heading(level: 1, spans: [.prose("Notes on C#")])])
}

@Test func blocksLeaveASevenHashLineAsProse() {
    // CommonMark stops at six; a longer run is a paragraph that starts with
    // hashes, not a heading.
    let blocks = JournalMarkdown.blocks("####### not a heading")
    #expect(blocks == [.paragraph([.prose("####### not a heading")])])
}

@Test func blocksReadAThematicBreakAfterAHeading() {
    // The shape the composer's own template uses: a heading, a rule under it,
    // then the list. The rule must not be read as the heading's underline.
    let blocks = JournalMarkdown.blocks("## The Highlights\n---\n- first")
    #expect(blocks == [
        .heading(level: 2, spans: [.prose("The Highlights")]),
        .rule,
        .bullets([[.prose("first")]]),
    ])
}

@Test func blocksReadDashesUnderAParagraphAsASetextHeading() {
    let blocks = JournalMarkdown.blocks("First Thoughts\n---\nA thought.")
    #expect(blocks == [
        .heading(level: 2, spans: [.prose("First Thoughts")]),
        .paragraph([.prose("A thought.")]),
    ])
}

@Test func blocksReadEqualsUnderAParagraphAsATopLevelHeading() {
    let blocks = JournalMarkdown.blocks("First Thoughts\n===")
    #expect(blocks == [.heading(level: 1, spans: [.prose("First Thoughts")])])
}

@Test func blocksTreatSpacedDashesAsARuleNotAnUnderline() {
    // `- - -` is a thematic break in CommonMark even under a paragraph, so the
    // paragraph survives it.
    let blocks = JournalMarkdown.blocks("A thought.\n- - -")
    #expect(blocks == [.paragraph([.prose("A thought.")]), .rule])
}

@Test func blocksReadEveryThematicBreakMarker() {
    let blocks = JournalMarkdown.blocks("---\n\n***\n\n___")
    #expect(blocks == [.rule, .rule, .rule])
}

@Test func blocksKeepEmphasisMarkersOffTheRulePath() {
    // `***bold***` opens with three stars but is not alone on the line.
    let blocks = JournalMarkdown.blocks("***bold***")
    #expect(blocks == [.paragraph([.prose("***bold***")])])
}

@Test func blocksGatherAQuote() {
    let blocks = JournalMarkdown.blocks("> a line\n> and another")
    #expect(blocks == [.quote([.prose("a line\nand another")])])
}

@Test func blocksKeepFencedCodeVerbatim() {
    let blocks = JournalMarkdown.blocks("```\nlet x = ||1||\n```")
    // No inline parse and no spoiler pass inside a fence.
    #expect(blocks == [.code("let x = ||1||")])
}

@Test func blocksKeepAnUnclosedFencesText() {
    let blocks = JournalMarkdown.blocks("```\nstill mine")
    #expect(blocks == [.code("still mine")])
}

@Test func blocksSurviveCarriageReturns() {
    // Splitting on a newline *set* would read CRLF as two breaks, and the blank
    // line between them would end the paragraph.
    let blocks = JournalMarkdown.blocks("One line\r\nand its wrap")
    #expect(blocks == [.paragraph([.prose("One line\nand its wrap")])])
}

@Test func blocksAreEmptyForAnEmptyBody() {
    #expect(JournalMarkdown.blocks("") == [])
    #expect(JournalMarkdown.blocks("\n  \n") == [])
}

// MARK: - Inline spoilers

@Test func spansSplitASpoilerOutOfItsLine() {
    var next = 0
    let spans = JournalMarkdown.spans("He was ||Ares|| all along", from: &next)
    #expect(spans == [.prose("He was "), .spoiler("Ares", id: 0), .prose(" all along")])
    #expect(next == 1)
}

@Test func spansNumberSpoilersInReadingOrderAcrossTheWholeEntry() {
    let blocks = JournalMarkdown.blocks("||one||\n\n- ||two||\n- ||three||")
    #expect(blocks == [
        .paragraph([.spoiler("one", id: 0)]),
        .bullets([[.spoiler("two", id: 1)], [.spoiler("three", id: 2)]]),
    ])
}

@Test func spansLeaveAnUnterminatedMarkerLiteral() {
    var next = 0
    let spans = JournalMarkdown.spans("Darrow hid ||that he trained with Lorn", from: &next)
    #expect(spans == [.prose("Darrow hid ||that he trained with Lorn")])
    #expect(next == 0)
}

@Test func spansNeverPairAMarkerAcrossALineBoundary() {
    // #2366: one stray marker must not invert every line after it. The line
    // that is short a marker keeps it literal; the next line still pairs.
    let blocks = JournalMarkdown.blocks("- hid ||in the gap|\n- killing ||Leto|| was smart")
    #expect(blocks == [.bullets([
        [.prose("hid ||in the gap|")],
        [.prose("killing "), .spoiler("Leto", id: 0), .prose(" was smart")],
    ])])
}

@Test func spansPairSeveralSpoilersOnOneLine() {
    var next = 0
    let spans = JournalMarkdown.spans(
        "During the ||Iron Rain||, the ||EMP blast|| landed", from: &next)
    #expect(spans == [
        .prose("During the "),
        .spoiler("Iron Rain", id: 0),
        .prose(", the "),
        .spoiler("EMP blast", id: 1),
        .prose(" landed"),
    ])
}

@Test func spansKeepInlineMarkupInsideASpoilerForTheInlineParser() {
    var next = 0
    let spans = JournalMarkdown.spans("Cannot believe ||**Fitchner** was ARES||", from: &next)
    #expect(spans == [.prose("Cannot believe "), .spoiler("**Fitchner** was ARES", id: 0)])
}

// MARK: - Masking

@Test func maskedCensorsEverySpoilerRegion() {
    let line = JournalMarkdown.masked("He was ||Ares|| and ||her uncle||")
    #expect(line == "He was \u{2588}\u{2588}\u{2588} and \u{2588}\u{2588}\u{2588}")
}

@Test func maskedLeavesProseAlone() {
    #expect(JournalMarkdown.masked("Wrote a C# parser") == "Wrote a C# parser")
}

// MARK: - Inline rendering

@Test func inlineStillResolvesBoldAndItalic() {
    let rendered = JournalMarkdown.inline("**Kvothe** is an *unreliable* narrator")
    #expect(String(rendered.characters) == "Kvothe is an unreliable narrator")
}

@Test func inlineKeepsTheLineBreaksTheSplitterLeftInABlock() {
    let rendered = JournalMarkdown.inline("one\ntwo")
    #expect(String(rendered.characters) == "one\ntwo")
}

// MARK: - Reveal links

@Test func revealURLRoundTripsItsSpoilerID() {
    let url = JournalMarkdown.revealURL(12)
    #expect(url.flatMap(JournalMarkdown.revealID) == 12)
}

@Test func revealIDIgnoresAnAuthorsOwnLinks() {
    #expect(JournalMarkdown.revealID(URL(string: "https://example.com/3")!) == nil)
}

// MARK: - The whole entry

@Test func blocksReadARealReviewEntryEndToEnd() {
    let entry = """
        ## First Thoughts
        There was never a dull moment in this book.

        I still can't figure out how ||The Jackal knew.||

        ## The Highlights
        ---
        - Bellona boys ||deserve everything they got.||
        - Cannot believe that ||Fitchner was **ARES**||...
        """
    let blocks = JournalMarkdown.blocks(entry)

    #expect(blocks.count == 6)
    #expect(blocks[0] == .heading(level: 2, spans: [.prose("First Thoughts")]))
    #expect(blocks[1] == .paragraph([.prose("There was never a dull moment in this book.")]))
    if case .paragraph(let spans) = blocks[2] {
        #expect(spans.contains(.spoiler("The Jackal knew.", id: 0)))
    } else {
        Issue.record("expected the spoiler line to be a paragraph")
    }
    #expect(blocks[3] == .heading(level: 2, spans: [.prose("The Highlights")]))
    #expect(blocks[4] == .rule)
    guard case .bullets(let items) = blocks[5] else {
        Issue.record("expected the dashed lines to gather into one list")
        return
    }
    #expect(items.count == 2)
    #expect(plain(items[0]) == "Bellona boys deserve everything they got.")
    #expect(plain(items[1]) == "Cannot believe that Fitchner was **ARES**...")
}
