//  BookDetailStopsTests.swift
//  The pure derivations behind the detail marquee's stops: the Home kicker,
//  the Resume label, the ruler fraction, the stats fold, and the caps the
//  Highlights and Journals stops apply before handing off to a sheet.

import Foundation
import Testing

@testable import omnibus

// MARK: - Kicker

@Test func kickerNamesSeriesBookAndYear() {
    let line = DetailRead.kicker(
        series: "Kingkiller Chronicle", seriesIndex: "1", fallback: "Fantasy", year: "2007"
    )
    #expect(line == "Kingkiller Chronicle · Book 1 · 2007")
}

@Test func kickerFallsBackToCategoryForStandalones() {
    let line = DetailRead.kicker(
        series: nil, seriesIndex: nil, fallback: "Fantasy", year: "2018"
    )
    #expect(line == "Fantasy · standalone · 2018")
}

@Test func kickerSurvivesABareRecord() {
    let line = DetailRead.kicker(series: nil, seriesIndex: nil, fallback: nil, year: nil)
    #expect(line == "In your library")
}

// MARK: - Resume label

@Test func resumeLabelSpeaksTheEbookPercentWhenOneIsSaved() {
    let label = DetailRead.resumeLabel(
        hasEbook: true, hasAudiobook: true, epubStarted: true,
        epubPercent: 55, audioSeconds: nil
    )
    #expect(label == "Resume — 55%")
}

@Test func resumeLabelSaysReadWhenNothingIsSaved() {
    let label = DetailRead.resumeLabel(
        hasEbook: true, hasAudiobook: false, epubStarted: false,
        epubPercent: nil, audioSeconds: nil
    )
    #expect(label == "Read")
}

@Test func resumeLabelSpeaksTheAudioPositionWhenOnlyListeningHasStarted() {
    // The dual-format case: reading never started, listening did.
    let label = DetailRead.resumeLabel(
        hasEbook: true, hasAudiobook: true, epubStarted: false,
        epubPercent: nil, audioSeconds: 55_260
    )
    #expect(label == "Resume — 15h 21m")
}

@Test func resumeLabelIsABareResumeForACFIOnlySave() {
    let label = DetailRead.resumeLabel(
        hasEbook: true, hasAudiobook: true, epubStarted: true,
        epubPercent: nil, audioSeconds: 55_260
    )
    #expect(label == "Resume")
}

@Test func resumeLabelSpeaksTheAudioPositionForAudioOnlyBooks() {
    let label = DetailRead.resumeLabel(
        hasEbook: false, hasAudiobook: true, epubStarted: false,
        epubPercent: nil, audioSeconds: 55_260
    )
    #expect(label == "Resume — 15h 21m")
}

@Test func resumeLabelSaysListenForAnUnstartedAudiobook() {
    let label = DetailRead.resumeLabel(
        hasEbook: false, hasAudiobook: true, epubStarted: false,
        epubPercent: nil, audioSeconds: 0
    )
    #expect(label == "Listen")
}

// MARK: - Resume destination

@Test func resumeOpensThePlayerWhenOnlyListeningHasStarted() {
    #expect(DetailRead.resumesIntoPlayer(
        hasEbook: true, hasAudiobook: true, epubStarted: false, audioSeconds: 900
    ))
}

@Test func resumeOpensTheReaderOnceReadingHasStarted() {
    #expect(!DetailRead.resumesIntoPlayer(
        hasEbook: true, hasAudiobook: true, epubStarted: true, audioSeconds: 900
    ))
}

@Test func resumeOpensTheReaderForAnUnstartedDualFormatBook() {
    #expect(!DetailRead.resumesIntoPlayer(
        hasEbook: true, hasAudiobook: true, epubStarted: false, audioSeconds: nil
    ))
}

@Test func resumeOpensThePlayerForAudioOnlyBooks() {
    #expect(DetailRead.resumesIntoPlayer(
        hasEbook: false, hasAudiobook: true, epubStarted: false, audioSeconds: nil
    ))
}

// MARK: - Ruler fraction

@Test func fractionComesFromTheEbookPercentWhenTheBookHasOne() {
    let fraction = DetailRead.fraction(
        epubStarted: true, epubPercent: 55, audioSeconds: nil, audioDuration: nil
    )
    #expect(fraction == 0.55)
}

@Test func fractionIsNilWhenACFIOnlySaveCarriesNoPercent() {
    // Reading is underway; the (older) audio position must not misplace it.
    let fraction = DetailRead.fraction(
        epubStarted: true, epubPercent: nil, audioSeconds: 120, audioDuration: 240
    )
    #expect(fraction == nil)
}

@Test func fractionFallsBackToAudioWhenReadingNeverStarted() {
    let fraction = DetailRead.fraction(
        epubStarted: false, epubPercent: nil, audioSeconds: 90, audioDuration: 360
    )
    #expect(fraction == 0.25)
}

@Test func fractionIsNilWhenAudioDurationIsUnknown() {
    let fraction = DetailRead.fraction(
        epubStarted: false, epubPercent: nil, audioSeconds: 90, audioDuration: nil
    )
    #expect(fraction == nil)
}

// MARK: - Stats fold

private func sitting(
    start: Int64, seconds: Int64, format: SessionFormat = .reading
) -> SessionLogEntry {
    SessionLogEntry(
        bookUUID: "b", title: "Book", format: format,
        startedAt: start, endedAt: start + seconds, seconds: seconds
    )
}

@Test func statsRecordFoldsTheSessionLog() {
    let now = Date(timeIntervalSince1970: 1_000_000)
    let record = DetailStats.record(
        from: [
            sitting(start: 900_000, seconds: 600),
            sitting(start: 100_000, seconds: 1_800),
            sitting(start: 500_000, seconds: 1_200, format: .listening),
        ],
        now: now
    )

    #expect(record?.startedAt == 100_000)
    #expect(record?.daysIn == 10)
    #expect(record?.totalSeconds == 3_600)
    #expect(record?.sessions == 3)
    #expect(record?.averageSeconds == 1_200)
    #expect(record?.longestSeconds == 1_800)
    #expect(record?.longestAt == 100_000)
    #expect(record?.readSeconds == 2_400)
    #expect(record?.listenSeconds == 1_200)
}

@Test func statsRecordIsNilWithNoSittings() {
    #expect(DetailStats.record(from: []) == nil)
}

@Test func sparkMinutesBucketsByCalendarDayOldestFirst() {
    // A fixed calendar so the day boundaries don't move with the test host.
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "UTC")!

    let now = Date(timeIntervalSince1970: 2_000_000)  // Jan 24 1970, 03:33 UTC
    let minutes = DetailStats.sparkMinutes(
        from: [
            // 30 minutes today, 10 minutes the previous calendar day —
            // late-evening Jan 23, which a trailing-24h window would misfile
            // as "today" — and one sitting far too old.
            sitting(start: 1_999_000, seconds: 1_800),
            sitting(start: 1_978_000, seconds: 600),
            sitting(start: 100, seconds: 6_000),
        ],
        days: 21,
        now: now,
        calendar: calendar
    )

    #expect(minutes.count == 21)
    #expect(minutes[20] == 30)
    #expect(minutes[19] == 10)
    #expect(minutes.reduce(0, +) == 40)
}

// MARK: - Stop caps

@Test func highlightsStopPreviewsTheNewestFour() {
    let highlights = (1...9).map { index in
        Highlight(
            id: Int64(index), bookUUID: "b", epubCFIRange: nil, color: .amber,
            note: nil, text: "line \(index)", clientID: nil, createdAt: Int64(index)
        )
    }
    let preview = StopHighlights.preview(of: highlights)

    #expect(preview.count == StopHighlights.stopCount)
    #expect(preview.map(\.id) == [9, 8, 7, 6])
}

@Test func highlightsStopPreviewKeepsAShortListWhole() {
    let highlights = [
        Highlight(
            id: 1, bookUUID: "b", epubCFIRange: nil, color: .amber,
            note: nil, text: "line", clientID: nil, createdAt: 5
        )
    ]
    #expect(StopHighlights.preview(of: highlights).count == 1)
}

@Test func journalRowPreviewStripsMarkdownFromTheOpeningLine() {
    let preview = JournalRow.preview("**Kvothe** is an *unreliable* narrator\n\nSecond para")
    #expect(preview == "Kvothe is an unreliable narrator")
}

@Test func journalRowPreviewKeepsProseCharactersThatLookLikeSyntax() {
    let preview = JournalRow.preview("Wrote a C# parser in snake_case style")
    #expect(preview == "Wrote a C# parser in snake_case style")
}

@Test func journalRowPreviewUnwrapsAListMarker() {
    let preview = JournalRow.preview("- The Cinder scene lands differently in audio")
    #expect(preview == "The Cinder scene lands differently in audio")
}

@Test func journalRowPreviewUnwrapsANumberedOrQuotedOpeningLine() {
    #expect(JournalRow.preview("1. Started it again") == "Started it again")
    #expect(JournalRow.preview("> The Beauty of the House") == "The Beauty of the House")
    // Prose that merely opens with a year keeps its digits — the marker needs
    // its `.` or `)`.
    #expect(JournalRow.preview("1984 reads differently now") == "1984 reads differently now")
}

@Test func journalRowPreviewCensorsASpoilerSoTheRowCannotLeakIt() {
    // The row is always visible, so the span never reaches it in the clear.
    let preview = JournalRow.preview("Cannot believe ||Fitchner was **ARES**|| honestly")
    #expect(preview == "Cannot believe \u{2588}\u{2588}\u{2588} honestly")
}


// MARK: - Two-position Home geometry

@Test func restTopSitsUnderAWholeCoverOnAPhone() {
    #expect(DetailRead.restTop(width: 402, height: 874) == 603)
}

@Test func restTopYieldsToAShortScreenSoThePanelKeepsItsStrip() {
    // A 375pt-wide cover runs 562pt tall; a 700pt screen caps the rest
    // position so the panel keeps its 240pt strip.
    #expect(DetailRead.restTop(width: 375, height: 700) == 460)
}

@Test func restTopKeepsAFloorOfArtOnADegenerateScreen() {
    #expect(DetailRead.restTop(width: 800, height: 400) == 220)
}

@Test func scrollMapLiftsAcrossTheRestRunBeforeAnyPageTurns() {
    let mid = DetailRead.scrollMap(offset: 300, restTop: 600, viewport: 874)
    #expect(abs(mid.lift - 0.5) < 0.001)
    #expect(mid.page == 0)

    let lifted = DetailRead.scrollMap(offset: 600, restTop: 600, viewport: 874)
    #expect(lifted.lift == 1)
    #expect(lifted.page == 0)
}

@Test func scrollMapCountsPagesPastTheRestRun() {
    let map = DetailRead.scrollMap(offset: 600 + 874 * 2, restTop: 600, viewport: 874)
    #expect(map.lift == 1)
    #expect(abs(map.page - 2) < 0.001)
}

@Test func scrollMapDegeneratesToPlainPagingWithoutARestRun() {
    let map = DetailRead.scrollMap(offset: 874, restTop: 0, viewport: 874)
    #expect(map.lift == 1)
    #expect(abs(map.page - 1) < 0.001)
}

// MARK: - Home sync row

@Test func syncRowInvitesALinkWhileFormatsAreNotLinked() {
    let copy = DetailSyncCopy(label: "Positions unlinked", action: "Link", linked: false)
    #expect(DetailRead.syncRow(state: .notLinked) == copy)
    // No answer yet (offline, or the fetch hasn't landed) reads the same —
    // the sheet it opens states the truth either way.
    #expect(DetailRead.syncRow(state: nil) == copy)
}

@Test func syncRowAsksForAReconfirmWhenTheLinkWentStale() {
    #expect(DetailRead.syncRow(state: .linkStale)
        == DetailSyncCopy(label: "Sync needs re-confirm", action: "Re-confirm", linked: false))
}

@Test func syncRowStatesLinkedForEveryLinkedState() {
    let linked = DetailSyncCopy(label: "Positions linked", action: "Manage", linked: true)
    #expect(DetailRead.syncRow(state: .aligned) == linked)
    #expect(DetailRead.syncRow(state: .candidate) == linked)
    #expect(DetailRead.syncRow(state: .nothingNewer) == linked)
}
