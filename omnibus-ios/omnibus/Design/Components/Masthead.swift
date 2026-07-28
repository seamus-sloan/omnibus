//  Masthead.swift
//  The editorial header the four tab roots lead with.
//
//  A stock `.navigationTitle` renders in system bold sans, which is the one
//  place the app was loudest and least itself — the web build's identity is an
//  italic Instrument Serif wordmark over warm neutrals. This is that identity,
//  set as a masthead: wordmark line, the screen's name in the display face, and
//  a rule with an accent lead-in.

import SwiftUI

struct Masthead<Trailing: View>: View {
    let title: String
    @ViewBuilder var trailing: () -> Trailing

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 7) {
                OmnibusMark()

                Text("Omnibus")
                    .font(.displayItalic(14))
                    .tracking(0.2)
                    .foregroundStyle(palette.ink2Color)

                Spacer(minLength: Spacing.sm)

                trailing()
            }

            Text(title)
                .font(.display(38))
                .foregroundStyle(palette.ink0Color)

            // The accent lead-in is the whole signature of the rule: a plain
            // hairline reads as a divider, this reads as a masthead.
            HStack(spacing: 0) {
                Rectangle()
                    .fill(palette.accentColor)
                    .frame(width: 28, height: 1.5)
                Rectangle()
                    .fill(palette.line2.color)
                    .frame(height: 0.5)
            }
        }
        .screenPadding()
        .padding(.top, Spacing.sm)
        .padding(.bottom, Spacing.lg)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

extension Masthead where Trailing == EmptyView {
    init(title: String) {
        self.init(title: title, trailing: { EmptyView() })
    }
}

/// The bar title on a screen that states its own name in a headline.
///
/// Printing it in the bar as well puts the same words on screen twice, 40pt
/// apart; leaving the bar empty strands you with no idea what you are looking
/// at once the headline scrolls away. This is the Apple Books behaviour: the
/// bar picks the name up exactly as the headline gives it up.
struct FadingBarTitle: ToolbarContent {
    let title: String
    let scrollY: CGFloat
    /// Offset at which the screen's own headline has cleared the bar.
    var appearsAfter: CGFloat = 64

    @Environment(\.palette) private var palette

    private var opacity: Double {
        Double(min(1, max(0, (scrollY - appearsAfter) / 44)))
    }

    var body: some ToolbarContent {
        ToolbarItem(placement: .principal) {
            Text(title)
                .font(.display(16, weight: .medium))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(1)
                .opacity(opacity)
        }
    }
}
