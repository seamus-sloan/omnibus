//  BookDetailFlow.swift
//  The shell pieces of the book detail's Option B — the marquee unrolled:
//  one snap stop (the cover, whole) and past it a single continuous list of
//  every section. The scroller and its content live in `BookDetailView`;
//  this file holds the standalone parts — the hero-region snap behavior,
//  the lift cue, the section rules, and the nav strip that replaces the
//  dot rail's chrome once the cover has gone.

import SwiftUI

/// The flow's snapping: only the hero region snaps — the whole cover, or
/// the body under the nav strip — and past a small overshoot the list
/// scrolls free. The design's `scroll-snap-type: proximity`, by hand.
struct FlowSnapBehavior: ScrollTargetBehavior {
    var restTop: CGFloat
    var navPeek: CGFloat = BookDetailView.flowNavPeek

    func updateTarget(_ target: inout ScrollTarget, context: TargetContext) {
        let lifted = max(1, restTop - navPeek)
        let y = target.rect.origin.y

        if y < lifted / 2 {
            target.rect.origin.y = 0
        } else if y < lifted + 60 {
            target.rect.origin.y = lifted
        }
    }
}

/// The flow's lift cue: the grab bar and its one-line invitation, shown
/// while the cover rests whole and gone once the list is up. Its space is
/// kept either way, so the body's first section doesn't jump.
struct FlowCue: View {
    let visible: Bool
    var onLift: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        Button {
            Haptics.tap()
            onLift()
        } label: {
            HStack(spacing: 9) {
                Capsule()
                    .fill(palette.ink2Color.opacity(0.55))
                    .frame(width: 36, height: 4)
                Text("SWIPE UP FOR EVERYTHING")
                    .font(.monoUI(8.5))
                    .tracking(1.4)
                    .foregroundStyle(palette.ink3Color)
                Spacer(minLength: 0)
            }
            .padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.bottom, 9)
        .opacity(visible ? 1 : 0)
        .allowsHitTesting(visible)
        .animation(Motion.snap, value: visible)
        .accessibilityLabel("Show everything")
        .accessibilityIdentifier("flow-lift-cue")
    }
}

/// The rule that introduces each section of the flow — its number, its
/// name, and a hairline running out to the edge.
struct FlowSectionLabel: View {
    let stop: DetailStop

    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: 10) {
            Text(String(format: "%02d", stop.rawValue + 1))
                .foregroundStyle(palette.ink3Color)
            Text(stop.name.uppercased())
                .foregroundStyle(palette.ink1Color)
            LinearGradient(
                colors: [palette.line2.color, .clear],
                startPoint: .leading,
                endPoint: .trailing
            )
            .frame(height: 1)
        }
        .font(.monoUI(9.5))
        .tracking(1.6)
        .padding(.bottom, 16)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// The blurred strip that fades in under the chrome discs once the cover
/// has gone — the flow has no full panel behind them, so this is what keeps
/// the status bar and discs legible over the running list.
struct FlowNavStrip: View {
    let shown: Bool

    @Environment(\.palette) private var palette

    var body: some View {
        Color.clear
            .frame(maxWidth: .infinity)
            // The strip ends level with the chrome discs; its background
            // runs on up under the status bar.
            .frame(height: 46)
            .background {
                ZStack {
                    Rectangle().fill(.ultraThinMaterial)
                    palette.bg0Color.opacity(0.78)
                }
                .ignoresSafeArea(edges: .top)
            }
            .overlay(alignment: .bottom) { Hairline() }
            .opacity(shown ? 1 : 0)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}
