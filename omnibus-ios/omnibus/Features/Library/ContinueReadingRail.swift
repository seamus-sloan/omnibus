//  ContinueReadingRail.swift
//  "Pick up where you left off" — a horizontal rail of resume points that
//  opens straight into the reader or the player.

import SwiftUI

struct ContinueReadingRail: View {
    let points: [ResumePoint]
    var onNavigate: (Destination) -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            Text("Continue")
                .font(.display(24))
                .foregroundStyle(palette.ink0Color)
                .screenPadding()

            ScrollView(.horizontal) {
                HStack(spacing: 14) {
                    ForEach(points) { point in
                        Button {
                            Haptics.tap()
                            open(point)
                        } label: {
                            ResumeCard(point: point)
                        }
                        .buttonStyle(BookPressStyle())
                    }
                }
                .screenPadding()
                .scrollTargetLayout()
            }
            .scrollIndicators(.hidden)
            .scrollTargetBehavior(.viewAligned)
        }
    }

    private func open(_ point: ResumePoint) {
        switch point.record.format {
        case .audio:
            // Reopen the file the position was taken in, not the book's
            // first one — two narrations don't share a timeline.
            Presentation.shared.openPlayer(point.book, fileID: point.record.bookFileID)
        case .epub:
            Presentation.shared.openReader(point.book)
        }
    }
}

/// A resume card tinted by the book's own extracted accent, so the rail reads
/// as "these specific books" rather than a row of identical grey chrome.
private struct ResumeCard: View {
    let point: ResumePoint
    @Environment(\.palette) private var palette

    private var accent: OKLCH { CoverIdentity(point.book).tone }

    /// Pinned to a fixed lightness so a pale cover and a dark one produce
    /// cards of the same visual weight.
    private var tint: Color {
        OKLCH(palette.bg0.l > 0.5 ? 0.92 : 0.26, accent.c * 0.55, accent.h).color
    }

    private var rule: Color {
        OKLCH(0.78, accent.c * 0.9, accent.h).color
    }

    var body: some View {
        HStack(spacing: 14) {
            BookCover(identity: CoverIdentity(point.book), size: .md)
                .frame(width: 62)
                .coverShadow(0.8)

            VStack(alignment: .leading, spacing: 4) {
                Text(point.book.displayTitle)
                    .font(.ui(14.5, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)

                Text(point.book.authorDisplay)
                    .font(.ui(11.5))
                    .foregroundStyle(palette.ink2Color)
                    .lineLimit(1)

                Spacer(minLength: 4)

                if let fraction = point.fraction {
                    ProgressBar(fraction: fraction, tint: rule)
                }

                HStack(spacing: 5) {
                    Image(systemName: point.record.format == .audio ? "headphones" : "book")
                        .font(.system(size: 9, weight: .bold))
                    Text(caption)
                        .font(.ui(10.5, weight: .medium))
                }
                .foregroundStyle(rule)
            }
            .frame(maxHeight: .infinity, alignment: .topLeading)
        }
        .padding(14)
        .frame(width: 256, height: 128, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous).fill(tint)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(.white.opacity(0.07), lineWidth: 0.5)
        )
    }

    private var caption: String {
        if point.record.format == .audio {
            if let chapter = point.chapterNumber, let count = point.chapterCount {
                return "Chapter \(chapter) of \(count)"
            }
            if let position = point.record.audioPositionSeconds {
                return Format.duration(position)
            }
            return "Listening"
        }
        if let fraction = point.fraction {
            return "\(Int(fraction * 100))% read"
        }
        return "Reading"
    }
}

/// A slim capsule track. `ProgressView`'s linear style renders at a fixed
/// height and can't be tinted per-item without fighting it.
struct ProgressBar: View {
    let fraction: Double
    var tint: Color
    var height: CGFloat = 3

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var filled = false

    var body: some View {
        GeometryReader { geometry in
            let clamped = min(1, max(0, fraction))
            let width = max(height, geometry.size.width * clamped)

            ZStack(alignment: .leading) {
                Capsule().fill(.white.opacity(0.14))

                Capsule()
                    // A flat bar states a number; the gradient and the lit cap
                    // make it read as distance covered.
                    .fill(
                        LinearGradient(
                            colors: [tint.opacity(0.75), tint],
                            startPoint: .leading,
                            endPoint: .trailing
                        )
                    )
                    .overlay(alignment: .trailing) {
                        Capsule()
                            .fill(.white.opacity(0.55))
                            .frame(width: height)
                            .blur(radius: 1)
                    }
                    .frame(width: filled ? width : height)
                    .shadow(color: tint.opacity(0.5), radius: 4, y: 0)
            }
            .onAppear {
                guard !filled else { return }
                guard !reduceMotion else {
                    filled = true
                    return
                }
                // Draws itself in on arrival, so the card announces where you
                // are rather than presenting a bar that was always there.
                withAnimation(Motion.page.delay(0.12)) { filled = true }
            }
        }
        .frame(height: height)
        .accessibilityElement()
        .accessibilityLabel("Progress")
        .accessibilityValue("\(Int(min(1, max(0, fraction)) * 100)) percent")
    }
}
