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

    @Test("elapsed and remaining labels share one wall-clock basis")
    func scrubberRowLabelsAgree() {
        // A 60-minute chapter at 2x, 20 book-minutes in: the row reads 10:00
        // elapsed and 20:00 left, summing to the 30-minute rate-adjusted
        // span. An elapsed label left in 1x book-time would show 20:00
        // elapsed beside 20:00 left at the chapter's one-third mark.
        let chapterDuration = 3600.0
        let offset = 1200.0
        let rate = 2.0
        #expect(Format.duration(Format.atRate(offset, rate: rate)) == "10:00")
        #expect(
            Format.duration(Format.atRate(chapterDuration - offset, rate: rate)) == "20:00")
        #expect(
            Format.atRate(offset, rate: rate)
                + Format.atRate(chapterDuration - offset, rate: rate)
                == Format.atRate(chapterDuration, rate: rate))
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
