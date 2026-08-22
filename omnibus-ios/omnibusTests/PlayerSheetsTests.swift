//  PlayerSheetsTests.swift
//  Pure logic behind the player's speed and sleep sheets.
//
//  The sheets mirror the web mobile player's SpeedSheet/SleepSheet, so these
//  tests mirror the web module's own (`sheets.rs` / `state.rs`): the rate
//  snap grid, the preset-highlight rules, and the remaining-time derivation
//  the countdown pill renders from.

import Testing

@testable import omnibus

@Suite("Playback speed snap")
struct PlaybackSpeedSnapTests {
    @Test("clamps to the rate bounds and snaps to 0.05 steps")
    func clampsAndSnaps() {
        #expect(abs(PlaybackSpeed.snap(0.1) - 0.5) < 1e-9)
        #expect(abs(PlaybackSpeed.snap(9.0) - 3.0) < 1e-9)
        #expect(abs(PlaybackSpeed.snap(1.23) - 1.25) < 1e-9)
        #expect(abs(PlaybackSpeed.snap(1.2) - 1.2) < 1e-9)
    }

    @Test("preset grid mirrors the web sheet's table")
    func presetsMatchWeb() {
        #expect(PlaybackSpeed.presets == [0.5, 0.8, 1.0, 1.1, 1.2, 1.5, 1.8, 2.0])
        // Every preset already sits on the fine-tune grid, so tapping one and
        // then nudging the stepper never jumps.
        for preset in PlaybackSpeed.presets {
            #expect(abs(PlaybackSpeed.snap(preset) - preset) < 1e-9)
        }
    }
}

@Suite("Sleep preset highlight")
struct SleepPresetHighlightTests {
    @Test("off highlights only the Off entry")
    func offHighlightsOff() {
        #expect(SleepPresets.isOn(.off, seconds: 0))
        #expect(!SleepPresets.isOn(.off, seconds: 900))
    }

    @Test("a countdown highlights the preset that armed it")
    func countdownHighlightsItsPreset() {
        let armed = SleepTimer.countdown(remaining: 100, preset: 900)
        #expect(SleepPresets.isOn(armed, seconds: 900))
        #expect(!SleepPresets.isOn(armed, seconds: 1800))
        #expect(!SleepPresets.isOn(armed, seconds: 0))
    }

    @Test("end of chapter highlights no preset")
    func endOfChapterHighlightsNothing() {
        let armed = SleepTimer.endOfChapter(atSeconds: 1.0)
        #expect(!SleepPresets.isOn(armed, seconds: 0))
        #expect(!SleepPresets.isOn(armed, seconds: 900))
    }

    @Test("preset table starts with Off and covers four hours")
    func presetTableMatchesWeb() {
        #expect(SleepPresets.all.first?.label == "Off")
        #expect(SleepPresets.all.first?.seconds == 0)
        #expect(SleepPresets.all.last?.label == "4 hours")
        #expect(SleepPresets.all.last?.seconds == 14400)
    }
}

@Suite("Sleep remaining")
struct SleepRemainingTests {
    @Test("is nil when off")
    func nilWhenOff() {
        #expect(AudioPlayer.sleepRemaining(.off, position: 100) == nil)
    }

    @Test("reads a countdown directly, clamped at zero")
    func readsCountdownDirectly() {
        let armed = SleepTimer.countdown(remaining: 120, preset: 900)
        #expect(AudioPlayer.sleepRemaining(armed, position: 0) == 120)
        let overshot = SleepTimer.countdown(remaining: -5, preset: 900)
        #expect(AudioPlayer.sleepRemaining(overshot, position: 0) == 0)
    }

    @Test("derives end of chapter from the playback position")
    func derivesEndOfChapterFromPosition() {
        let armed = SleepTimer.endOfChapter(atSeconds: 500)
        #expect(AudioPlayer.sleepRemaining(armed, position: 380) == 120)
        // Past the boundary clamps to zero rather than going negative.
        #expect(AudioPlayer.sleepRemaining(armed, position: 900) == 0)
    }
}
