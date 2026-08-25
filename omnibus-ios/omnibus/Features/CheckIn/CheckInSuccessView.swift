//  CheckInSuccessView.swift
//  The terminal screen after a check-in flow write lands. Celebration tone
//  gets confetti + rings around the cover; quiet tone (wishlist) gets rings
//  only. Both funnel back to the scanner via "Scan another".

import SwiftUI

struct CheckInSuccessView: View {
    let success: CheckInSuccess
    var onScanAnother: () -> Void
    var onViewBook: (String) -> Void

    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var revealed = false

    private static let coverWidth: CGFloat = 130
    private static let ringDiameter: CGFloat = 236

    var body: some View {
        ZStack {
            if success.tone == .celebration {
                ConfettiView(accent: palette.accentColor)
            }

            VStack(spacing: 0) {
                Spacer(minLength: Spacing.lg)

                ZStack {
                    // Owning a book keeps celebrating; a wishlist add pulses
                    // once and settles.
                    if !reduceMotion {
                        if success.tone == .celebration {
                            haloRing(delay: 0, repeats: true)
                            haloRing(delay: 0.55, repeats: true)
                        } else {
                            haloRing(delay: 0, repeats: false)
                        }
                    }
                    Circle()
                        .stroke(palette.accentColor, lineWidth: 2)
                        .frame(width: Self.ringDiameter, height: Self.ringDiameter)
                        .shadow(color: palette.accentColor.opacity(0.45), radius: 17)
                        .scaleEffect(revealed || reduceMotion ? 1 : 0.55)
                        .opacity(revealed || reduceMotion ? 1 : 0)
                    coverArt
                        .frame(width: Self.coverWidth)
                }
                .frame(height: Self.ringDiameter + Spacing.lg)

                VStack(spacing: Spacing.sm) {
                    Text(success.headline.uppercased())
                        .font(.monoUI(10, weight: .semibold))
                        .tracking(0.8)
                        .foregroundStyle(palette.accentColor)
                    Text(success.title)
                        .font(.display(27))
                        .foregroundStyle(palette.ink0Color)
                        .multilineTextAlignment(.center)
                    if success.tone == .quiet {
                        Text("Wishlisting only tracks this book — check in a copy or add a file to move it into your library.")
                            .font(.ui(12.5))
                            .foregroundStyle(palette.ink2Color)
                            .multilineTextAlignment(.center)
                    }
                }
                .padding(.top, Spacing.xl)
                .opacity(revealed || reduceMotion ? 1 : 0)
                .offset(y: revealed || reduceMotion ? 0 : 8)

                Spacer(minLength: Spacing.lg)

                VStack(spacing: Spacing.sm) {
                    Button("Scan another", action: onScanAnother)
                        .buttonStyle(FilledButtonStyle())
                    if let uuid = success.bookUUID {
                        Button {
                            onViewBook(uuid)
                        } label: {
                            // QuietButtonStyle hugs its label; stretching the
                            // label matches the primary button's width.
                            Text("View book").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(QuietButtonStyle())
                    }
                }
            }
            .screenPadding()
            .padding(.bottom, Spacing.lg)
        }
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(Motion.lift) { revealed = true }
        }
    }

    /// An expanding, fading pulse centered on the cover — looping for the
    /// celebration tone, a single pulse for the quiet one.
    private func haloRing(delay: Double, repeats: Bool) -> some View {
        HaloRing(delay: delay, repeats: repeats, accent: palette.accentColor)
            .frame(width: Self.ringDiameter, height: Self.ringDiameter)
    }

    @ViewBuilder
    private var coverArt: some View {
        switch success.cover {
        case let .library(uuid):
            BookCover(
                identity: CoverIdentity(uuid: uuid, title: success.title, hasCover: true),
                size: .lg
            )
        case let .external(url):
            ExternalImage(url: url) { letterPlate }
                .aspectRatio(2.0 / 3.0, contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: Radius.md, style: .continuous))
        case .plate:
            letterPlate
        }
    }

    private var letterPlate: some View {
        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
            .fill(palette.coverFallbackBg.color)
            .aspectRatio(2.0 / 3.0, contentMode: .fit)
            .overlay {
                Text(success.title.prefix(1).uppercased())
                    .font(.display(34))
                    .foregroundStyle(palette.coverFallbackInk.color.opacity(0.5))
            }
    }
}

/// One halo pulse. Its animation state is private so each ring can run its
/// own clock with a distinct phase offset. A pulse ends fully expanded at
/// zero opacity, so a non-repeating ring simply vanishes after one pass.
private struct HaloRing: View {
    var delay: Double
    var repeats: Bool
    var accent: Color

    @State private var expanded = false

    var body: some View {
        Circle()
            .stroke(accent, lineWidth: 2)
            .scaleEffect(expanded ? 1.85 : 0.62)
            .opacity(expanded ? 0 : 0.55)
            .onAppear {
                let pulse = Animation.easeOut(duration: 1.8).delay(delay)
                withAnimation(repeats ? pulse.repeatForever(autoreverses: false) : pulse) {
                    expanded = true
                }
            }
    }
}
