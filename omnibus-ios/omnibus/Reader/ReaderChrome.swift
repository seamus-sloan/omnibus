//  ReaderChrome.swift
//  The reader's floating controls: the round page buttons, the bottom menu and
//  its rows, and the "back to page" return pill.
//
//  Apple Books keeps the reading view almost empty — a close button, two
//  centred labels, one menu button — and puts everything else behind that menu.
//  The page keeps its full measure and the controls read as hovering above the
//  prose rather than framing it, which matters most on the one screen where the
//  content is supposed to be the entire interface.

import SwiftUI

/// Shared metrics for the bottom menu, so the rows and the quick-action strip
/// line up as one column.
enum ReaderMenu {
    static let width: CGFloat = 272
    static let rowHeight: CGFloat = 46
    static let spacing: CGFloat = 8
    /// Buttons line up with the text column rather than the screen edge, which
    /// is what stops them reading as stuck to the bezel.
    static let inset: CGFloat = 28
    static let buttonSize: CGFloat = 46
}

/// A lone round floating control.
///
/// The glass goes on the *label*, inside the button. Wrapped around the button
/// instead — especially as `.interactive()` — it takes the touch itself and the
/// button's action never runs.
struct ReaderGlassButton: View {
    let icon: String
    let label: String
    let ink: Color
    var diameter: CGFloat = 44
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            Image(systemName: icon)
                .font(.system(size: diameter * 0.4, weight: .medium))
                .foregroundStyle(ink)
                .frame(width: diameter, height: diameter)
                .glassEffect(.regular, in: .circle)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

/// One row of the reader menu.
///
/// The trailing glyph is optional: a row that already ends in a count doesn't
/// need one, and on the longest label the two together push the title onto a
/// second line.
struct ReaderMenuRow: View {
    let title: String
    var icon: String?
    var count: Int?
    let ink: Color
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            HStack(spacing: Spacing.sm) {
                Text(title)
                    .font(.ui(16.5))
                    .lineLimit(1)
                Spacer(minLength: Spacing.sm)
                if let count, count > 0 {
                    Text("\(count)")
                        .font(.ui(16))
                        .foregroundStyle(ink.opacity(0.6))
                        .monospacedDigit()
                }
                if let icon {
                    Image(systemName: icon)
                        .font(.system(size: 17, weight: .regular))
                }
            }
            .foregroundStyle(ink)
            .padding(.horizontal, 18)
            .frame(width: ReaderMenu.width, height: ReaderMenu.rowHeight)
            .glassEffect(.regular, in: .capsule)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

/// The Contents row, which doubles as the book scrubber.
///
/// Press and hold, then slide — the way Apple Books hides seeking behind the
/// row that already reports where you are, instead of spending a permanent
/// slider on it. A plain tap opens the contents.
///
/// The seek commits on release: epub.js re-paginates on every `display`, so
/// seeking each intermediate position turns the slide into a stutter and lands
/// you somewhere you never aimed at. The percentage tracks the finger so the
/// gesture still reads as continuous.
struct ReaderScrubRow: View {
    let fraction: Double
    let ink: Color
    let onOpen: () -> Void
    /// Called once when a scrub begins, so the host can remember where you were.
    let onScrubStart: () -> Void
    /// The live position while scrubbing, and `nil` on release — drives the
    /// readout above the row.
    var onScrubChange: (Double?) -> Void = { _ in }
    let onSeek: (Double) -> Void

    /// Set while scrubbing and until the reader reports its new position —
    /// snapping back to the old percentage on release reads as a failed drag.
    @State private var scrubbed: Double?
    @State private var isScrubbing = false
    @State private var pressStart: Date?
    /// Where the finger was when the hold engaged, so the value doesn't jump by
    /// however far you had already drifted.
    @State private var anchor: CGFloat = 0
    @State private var base: Double = 0

    private static let holdDuration: TimeInterval = 0.28

    private var shown: Double { scrubbed ?? fraction }
    private var percent: Int { Int((shown * 100).rounded()) }

    var body: some View {
        ZStack {
            // Scrubbing turns the row into the track it is acting as — the
            // label would be describing a position the finger has already left.
            if isScrubbing {
                track
            } else {
                HStack(spacing: 0) {
                    Text("Contents")
                        .font(.ui(16.5))
                    Text(" · \(percent)%")
                        .font(.ui(16.5))
                        .foregroundStyle(ink.opacity(0.6))
                        .contentTransition(.numericText())
                    Spacer(minLength: Spacing.sm)
                    Image(systemName: "list.bullet")
                        .font(.system(size: 17, weight: .regular))
                }
                .padding(.horizontal, 18)
            }
        }
        .foregroundStyle(ink)
        .frame(width: ReaderMenu.width, height: ReaderMenu.rowHeight)
        .glassEffect(.regular, in: .capsule)
        .contentShape(Capsule())
        .scaleEffect(isScrubbing ? 1.03 : 1)
        .animation(Motion.snap, value: isScrubbing)
        .gesture(gesture)
        .onChange(of: fraction) { _, _ in scrubbed = nil }
        .accessibilityElement()
        .accessibilityLabel("Contents")
        .accessibilityValue("\(percent) percent through the book")
        .accessibilityHint("Double tap to open contents. Adjust to move through the book.")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { onOpen() }
        .accessibilityAdjustableAction { direction in
            let next = direction == .increment ? shown + 0.02 : shown - 0.02
            onSeek(min(1, max(0, next)))
        }
    }

    /// The filled portion behind a marker at the current position — the only
    /// cue that a slide is doing anything before the page catches up.
    private var track: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            ZStack(alignment: .leading) {
                Rectangle()
                    .fill(ink.opacity(0.14))
                    .frame(width: max(0, width * shown))
                Rectangle()
                    .fill(ink.opacity(0.85))
                    .frame(width: 2)
                    .offset(x: min(max(width * shown - 1, 0), width - 2))
            }
        }
        .allowsHitTesting(false)
    }

    /// One gesture serves both verbs, so a tap and a hold can't fight over the
    /// touch: a hold that then moves scrubs, anything shorter opens contents.
    private var gesture: some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                if pressStart == nil { pressStart = Date() }
                guard let start = pressStart else { return }

                if !isScrubbing {
                    guard Date().timeIntervalSince(start) >= Self.holdDuration else { return }
                    isScrubbing = true
                    anchor = value.translation.width
                    base = fraction
                    scrubbed = fraction
                    onScrubStart()
                    onScrubChange(fraction)
                    Haptics.select()
                    return
                }

                let travelled = (value.translation.width - anchor) / ReaderMenu.width
                let next = min(1, max(0, base + Double(travelled)))
                scrubbed = next
                onScrubChange(next)
            }
            .onEnded { value in
                let held = pressStart.map { Date().timeIntervalSince($0) } ?? 0
                pressStart = nil

                if isScrubbing {
                    isScrubbing = false
                    onScrubChange(nil)
                    guard let target = scrubbed else { return }
                    Haptics.tap()
                    onSeek(target)
                    return
                }

                let stationary = abs(value.translation.width) < 10
                    && abs(value.translation.height) < 10
                if stationary, held < 0.6 {
                    Haptics.tap()
                    onOpen()
                }
            }
    }
}

/// The way back from a jump — a round arrow and the page you left.
///
/// Lives top-left, opposite the close button, rather than as a banner over the
/// prose: it needs to persist long enough to be useful, and anything wider than
/// this would be sitting on the page you're trying to read.
struct ReaderReturnButton: View {
    let page: Int
    let ink: Color
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            HStack(spacing: 7) {
                Image(systemName: "arrow.uturn.backward")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(ink)
                    .frame(width: 30, height: 30)
                    .glassEffect(.regular, in: .circle)
                Text("\(page)")
                    .font(.ui(15, weight: .medium))
                    .foregroundStyle(ink.opacity(0.75))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Back to page \(page)")
    }
}

/// What a scrub is pointing at: the chapter you'd land in, and its page.
struct ReaderScrubReadout: View {
    let chapter: String?
    let page: Int?
    let ink: Color

    var body: some View {
        VStack(spacing: 1) {
            if let chapter {
                Text(chapter)
                    .font(.ui(16, weight: .semibold))
                    .lineLimit(1)
            }
            if let page {
                Text("Page \(page)")
                    .font(.ui(14))
                    .foregroundStyle(ink.opacity(0.7))
            }
        }
        .foregroundStyle(ink)
        .frame(width: ReaderMenu.width)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: Radius.xl))
        .allowsHitTesting(false)
    }
}
