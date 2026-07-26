//  ContinueHero.swift
//  Resuming a book is the most frequent thing anyone does in a reading app, so
//  it gets the top of the screen and a real target rather than a card in a rail.
//
//  Multiple books in progress page horizontally in one card's worth of space —
//  a second rail would cost another band of the landing screen for something
//  most people only have two or three of.

import SwiftUI

struct ContinueHero: View {
    let points: [ResumePoint]

    @Environment(\.palette) private var palette
    @State private var selection = 0

    private var height: CGFloat { 176 }

    var body: some View {
        VStack(spacing: 10) {
            TabView(selection: $selection) {
                ForEach(Array(points.enumerated()), id: \.element.id) { index, point in
                    HeroCard(point: point)
                        .padding(.horizontal, Spacing.screen)
                        .tag(index)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            .frame(height: height)

            if points.count > 1 {
                HStack(spacing: 6) {
                    ForEach(points.indices, id: \.self) { index in
                        Capsule()
                            .fill(index == selection ? palette.ink1Color : palette.ink3Color.opacity(0.4))
                            .frame(width: index == selection ? 16 : 5, height: 5)
                            .animation(Motion.snap, value: selection)
                    }
                }
            }
        }
    }
}

private struct HeroCard: View {
    let point: ResumePoint

    @Environment(\.palette) private var palette

    private var book: Book { point.book }
    private var tone: OKLCH { CoverIdentity(book).tone }
    private var isAudio: Bool { point.record.format == .audio }

    private var washTone: OKLCH {
        OKLCH(palette.bg0.l > 0.5 ? 0.93 : 0.24, tone.c * 0.6, tone.h)
    }

    private var rule: Color {
        OKLCH(0.80, tone.c * 0.95, tone.h).color
    }

    var body: some View {
        Button {
            Haptics.tap()
            if isAudio {
                // Reopen the file the position was taken in, not the book's
                // first one — two narrations don't share a timeline.
                Presentation.shared.openPlayer(book, fileID: point.record.bookFileID)
            } else {
                Presentation.shared.openReader(book)
            }
        } label: {
            HStack(alignment: .top, spacing: 16) {
                BookCover(identity: CoverIdentity(book), size: .md, cornerRadius: 6)
                    .frame(width: 96)
                    .coverShadow(1.2)

                VStack(alignment: .leading, spacing: 5) {
                    Text(isAudio ? "Continue listening" : "Continue reading")
                        .font(.ui(10.5, weight: .semibold))
                        .tracking(0.6)
                        .textCase(.uppercase)
                        .foregroundStyle(rule)

                    Text(book.displayTitle)
                        .font(.display(21))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)

                    Text(book.authorDisplay)
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink2Color)
                        .lineLimit(1)

                    Spacer(minLength: 4)

                    // An EPUB resume point carries a CFI, not a percentage, so
                    // there is no honest bar to draw for one. The band the bar
                    // would occupy goes to when you left off instead of sitting
                    // empty — which is what made the card read as underfilled.
                    if let fraction = point.fraction {
                        ProgressBar(fraction: fraction, tint: rule)
                    } else {
                        // Sentence case, quietly: the eyebrow above is already
                        // set in caps, and two shouting lines in one card read
                        // as a warning rather than as a detail.
                        Text(lastOpenedLabel)
                            .font(.ui(11.5))
                            .foregroundStyle(palette.ink3Color)
                    }

                    HStack(spacing: 8) {
                        Text(positionLabel)
                            .font(.ui(11.5, weight: .medium))
                            .foregroundStyle(palette.ink2Color)

                        Spacer(minLength: 0)

                        HStack(spacing: 5) {
                            Image(systemName: isAudio ? "play.fill" : "book.fill")
                                .font(.system(size: 10, weight: .bold))
                            Text(isAudio ? "Play" : "Read")
                                .font(.ui(12, weight: .semibold))
                        }
                        .foregroundStyle(palette.accentInk.color)
                        .padding(.horizontal, 13)
                        .padding(.vertical, 7)
                        .background(Capsule().fill(palette.accentColor))
                    }
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(cardGround)
            .overlay(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .strokeBorder(
                        LinearGradient(
                            colors: [.white.opacity(0.16), .white.opacity(0.04)],
                            startPoint: .top,
                            endPoint: .bottom
                        ),
                        lineWidth: 0.5
                    )
            )
            // Grounds the card against the page instead of letting it float as
            // a flat panel of colour.
            .shadow(color: .black.opacity(0.35), radius: 18, y: 8)
        }
        .buttonStyle(BookPressStyle())
    }

    /// A lit alcove rather than a filled rectangle: the wash graded top-to-
    /// bottom, with a bloom of the book's own colour behind its cover.
    private var cardGround: some View {
        RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
            .fill(
                LinearGradient(
                    colors: [
                        washTone.color,
                        OKLCH(washTone.l * 0.78, washTone.c * 0.9, tone.h).color,
                    ],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            )
            .overlay(alignment: .leading) {
                // `endRadius` must land inside the frame's half-width, or the
                // frame clips the gradient while it still has colour and the
                // bloom shows a hard vertical seam down the card.
                RadialGradient(
                    colors: [OKLCH(0.62, tone.c * 0.9, tone.h).color.opacity(0.40), .clear],
                    center: .center,
                    startRadius: 0,
                    endRadius: 160
                )
                .frame(width: 320, height: 320)
                .offset(x: -60)
                .blendMode(.plusLighter)
            }
            .clipShape(RoundedRectangle(cornerRadius: Radius.lg, style: .continuous))
    }

    private var positionLabel: String {
        if isAudio {
            if let chapter = point.chapterNumber, let count = point.chapterCount {
                return "Chapter \(chapter) of \(count)"
            }
            if let total = point.totalDurationSeconds,
               let position = point.record.audioPositionSeconds {
                return Format.humanDuration(Int64(total - position)) + " left"
            }
            return "In progress"
        }
        if let fraction = point.fraction {
            return "\(Int(fraction * 100))% read"
        }
        return isAudio ? "Listening" : "Reading"
    }

    private var lastOpenedLabel: String {
        "Last opened \(Format.relative(unix: point.record.updatedAt))"
    }
}
