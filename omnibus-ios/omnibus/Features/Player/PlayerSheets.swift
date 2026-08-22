//  PlayerSheets.swift
//  Speed and sleep bottom sheets for the audiobook player.
//
//  Mirrors the web mobile player's SpeedSheet/SleepSheet
//  (frontend/src/pages/listen/mobile/sheets.rs): same preset tables,
//  fine-tune step, and end-of-chapter option, so the two clients' player
//  modals can't drift.

import SwiftUI

/// Playback-speed constants shared by the sheet's grid, slider, and stepper.
enum PlaybackSpeed {
    /// Preset grid shown in the sheet, mirroring the web sheet's presets.
    static let presets: [Double] = [0.5, 0.8, 1.0, 1.1, 1.2, 1.5, 1.8, 2.0]
    /// Rate bounds, matching `MIN`/`MAX_AUDIOBOOK_PLAYBACK_RATE` on the wire.
    static let minRate = 0.5
    static let maxRate = 3.0
    /// Fine-tune slider step (also the ± stepper increment).
    static let step = 0.05

    /// Clamp + snap a requested rate to the fine-tune grid.
    static func snap(_ rate: Double) -> Double {
        let clamped = min(maxRate, max(minRate, rate))
        return (clamped / step).rounded() * step
    }
}

/// Playback-speed sheet: preset grid, fine-tune slider, and a ±0.05 stepper.
struct SpeedSheet: View {
    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            SheetHead(title: "Playback speed") {
                Text(String(format: "%.2f×", player.rate))
                    .font(.monoUI(15, weight: .semibold))
                    .foregroundStyle(palette.accentColor)
            }

            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: Spacing.sm), count: 4), spacing: Spacing.sm) {
                ForEach(PlaybackSpeed.presets, id: \.self) { preset in
                    SheetOption(
                        label: String(format: "%.1f×", preset),
                        mono: true,
                        on: abs(preset - player.rate) < 0.001
                    ) { player.rate = preset }
                }
            }

            VStack(alignment: .leading, spacing: Spacing.xs) {
                HStack {
                    Text("Fine-tune")
                    Spacer()
                    Text("0.5× — 3.0×")
                }
                .font(.ui(11))
                .foregroundStyle(palette.ink3Color)

                Slider(
                    value: rateBinding,
                    in: PlaybackSpeed.minRate...PlaybackSpeed.maxRate,
                    step: PlaybackSpeed.step
                ) { editing in
                    if !editing { player.commitRate() }
                }
                .tint(palette.accentColor)
            }

            HStack(spacing: Spacing.md) {
                stepButton("−", accessibility: "Slower") {
                    player.rate = PlaybackSpeed.snap(player.rate - PlaybackSpeed.step)
                }
                VStack(spacing: 2) {
                    Text(String(format: "%.2f×", player.rate))
                        .font(.monoUI(15, weight: .semibold))
                        .foregroundStyle(palette.ink0Color)
                    Text("0.05 steps")
                        .font(.ui(11))
                        .foregroundStyle(palette.ink3Color)
                }
                .frame(maxWidth: .infinity)
                stepButton("+", accessibility: "Faster") {
                    player.rate = PlaybackSpeed.snap(player.rate + PlaybackSpeed.step)
                }
            }
        }
        .sheetFrame()
    }

    // Live-only during the drag; the slider's onEditingChanged persists once
    // on release rather than once per 0.05 tick.
    private var rateBinding: Binding<Double> {
        Binding(
            get: { player.rate },
            set: { player.setRateLive(PlaybackSpeed.snap($0)) }
        )
    }

    private func stepButton(_ glyph: String, accessibility: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(glyph)
                .font(.monoUI(19, weight: .semibold))
                .foregroundStyle(palette.ink0Color)
                .frame(width: 64, height: 44)
                .background(RoundedRectangle(cornerRadius: 10).fill(palette.bg2Color))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(accessibility)
    }
}

/// Sleep-preset table shown in the sheet grid. `0` seconds == "Off".
/// Mirrors the web sheet's `SLEEP_PRESETS`.
enum SleepPresets {
    static let all: [(label: String, seconds: Int)] = [
        ("Off", 0),
        ("15 min", 900),
        ("30 min", 1800),
        ("45 min", 2700),
        ("1 hour", 3600),
        ("2 hours", 7200),
        ("3 hours", 10800),
        ("4 hours", 14400),
    ]

    /// Whether a preset button should render highlighted for the current
    /// state. Mirrors the web sheet's `sleep_preset_on`.
    static func isOn(_ timer: SleepTimer, seconds: Int) -> Bool {
        switch timer {
        case .off:
            return seconds == 0
        case .countdown(_, let preset):
            return preset == seconds
        case .endOfChapter:
            return false
        }
    }
}

/// Sleep-timer sheet: preset grid plus an "End of chapter" option, with the
/// live remaining countdown in the header while armed.
struct SleepSheet: View {
    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            SheetHead(title: "Sleep timer") {
                if let remaining = player.sleepRemainingSeconds, remaining > 0 {
                    VStack(alignment: .trailing, spacing: 2) {
                        Text(Format.duration(Double(remaining)))
                            .font(.monoUI(15, weight: .semibold))
                            .foregroundStyle(palette.accentColor)
                        Text("remaining")
                            .font(.ui(11))
                            .foregroundStyle(palette.ink3Color)
                    }
                }
            }

            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: Spacing.sm), count: 2), spacing: Spacing.sm) {
                ForEach(SleepPresets.all, id: \.seconds) { preset in
                    SheetOption(
                        label: preset.label,
                        on: SleepPresets.isOn(player.sleepTimer, seconds: preset.seconds)
                    ) { player.startSleepTimer(seconds: preset.seconds) }
                }
            }

            if let index = player.currentChapterIndex, player.chapterDuration > 0 {
                SheetOption(label: "End of chapter \(index + 1)", on: endOfChapterArmed) {
                    player.startSleepTimer(
                        endOfChapterAt: player.chapterStart + player.chapterDuration
                    )
                }
            }
        }
        .sheetFrame()
    }

    private var endOfChapterArmed: Bool {
        if case .endOfChapter = player.sleepTimer { return true }
        return false
    }
}

// MARK: - Shared sheet chrome

/// Title row with an optional right-side meta element, the sheets' shared
/// counterpart of the web `MSheet` head.
private struct SheetHead<Meta: View>: View {
    @Environment(\.palette) private var palette

    let title: String
    @ViewBuilder let meta: Meta

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.ui(17, weight: .semibold))
                .foregroundStyle(palette.ink0Color)
            Spacer()
            meta
        }
    }
}

/// One tappable option in a sheet grid — highlighted with the accent while
/// its value is the active one.
private struct SheetOption: View {
    @Environment(\.palette) private var palette

    let label: String
    var mono = false
    let on: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(label)
                .font(mono ? .monoUI(14, weight: .medium) : .ui(14, weight: .medium))
                .foregroundStyle(on ? palette.accentColor : palette.ink1Color)
                .frame(maxWidth: .infinity, minHeight: 44)
                .background(
                    RoundedRectangle(cornerRadius: 10)
                        .fill(on ? palette.accentColor.opacity(0.14) : palette.bg2Color)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 10)
                        .strokeBorder(on ? palette.accentColor : .clear, lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
    }
}

extension View {
    /// Shared frame for the player's bottom sheets: padding, screen
    /// background, and a half-height detent with the grabber visible —
    /// the same chrome the app's other sheets carry.
    fileprivate func sheetFrame() -> some View {
        padding(.horizontal, Spacing.screen)
            .padding(.top, Spacing.xl)
            .padding(.bottom, Spacing.xl)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(ScreenBackground())
            .presentationDetents([.medium])
            .presentationDragIndicator(.visible)
    }
}
