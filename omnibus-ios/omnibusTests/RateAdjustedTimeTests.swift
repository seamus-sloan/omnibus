//  RateAdjustedTimeTests.swift
//  Rate-adjusted time math behind the player's "left" labels and stats.
//
//  `Format.atRate` converts book-time seconds into the wall-clock time a
//  listener at a given playback rate actually experiences. Two consumers
//  depend on it staying exact: the player's scrubber-row readouts (elapsed
//  and both remaining labels), and the session tracker's per-tick accrual
//  (the periodic observer fires in media time, so each 0.5s tick is divided
//  by the rate before it counts toward listening stats).

import Testing

@testable import omnibus

@Suite("Rate-adjusted time")
struct RateAdjustedTimeTests {
    @Test("divides remaining seconds by the playback rate")
    func dividesByRate() {
        #expect(Format.atRate(600, rate: 2.0) == 300)
        #expect(Format.atRate(600, rate: 0.5) == 1200)
        #expect(Format.atRate(600, rate: 1.0) == 600)
    }

    @Test("falls back to the unscaled value for invalid rates")
    func fallsBackForInvalidRates() {
        #expect(Format.atRate(600, rate: 0) == 600)
        #expect(Format.atRate(600, rate: -1) == 600)
        #expect(Format.atRate(600, rate: .nan) == 600)
        #expect(Format.atRate(600, rate: .infinity) == 600)
    }

    @Test("the position is book time and only the time left is wall clock")
    func scrubberRowLabelsFollowTheSettledConvention() {
        // Replaces an earlier test that asserted the opposite — that every
        // figure in the row shared one wall-clock basis, so they summed to
        // the rate-adjusted span. That was #2246's rule; #2344 repealed it
        // on the web because rescaling a *position* made it disagree with
        // the bookmark stamps and the detail page that name the same spot.
        // iOS kept the old rule until #2521, where a transport reading 22:57
        // sat beside a contents panel calling the same chapter 1:16:11.
        //
        // A 60-minute chapter at 2x, 20 book-minutes in: the position reads
        // 20:00 — the same string at any speed — and the time left reads
        // 20:00 of wall clock, which is what the listener will actually wait.
        let chapterDuration = 3600.0
        let offset = 1200.0
        let rate = 2.0

        // The position formatter takes no rate at all — that is the design,
        // and it cannot be demonstrated by feeding rates to a function that
        // does not accept one. Pin it by contrast instead: the rate-adjusted
        // helper the transport used to apply here really does move, so
        // reaching for the plain formatter is a choice with consequences.
        #expect(Format.duration(offset) == "20:00")
        for anyRate in [1.2, 1.5, 2.0] {
            #expect(
                Format.duration(Format.atRate(offset, rate: anyRate)) != "20:00",
                "atRate must move the position at \(anyRate)x, or this proves nothing")
        }

        // The time left is rate-adjusted, and says so.
        #expect(
            Format.duration(Format.atRate(chapterDuration - offset, rate: rate)) == "20:00")
        #expect(
            Format.duration(Format.atRate(chapterDuration - offset, rate: 1.0)) == "40:00")

        // The two no longer sum to the span, and must not be read as though
        // they did — which is why both carry a label on screen.
        #expect(
            offset + Format.atRate(chapterDuration - offset, rate: rate)
                != Format.atRate(chapterDuration, rate: rate))
    }

    @Test("a media-time observer tick accrues wall-clock stats time")
    func observerTickAccruesWallClock() {
        // One 0.5s media tick at 2x is a quarter second of real listening;
        // an hour of 2x playback must total 30 wall-clock minutes.
        #expect(Format.atRate(0.5, rate: 2.0) == 0.25)
        let ticksPerBookHour = 7200.0
        #expect(Format.atRate(0.5, rate: 2.0) * ticksPerBookHour == 1800)
    }
}
