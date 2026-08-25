//  BookHero.swift
//  The jacket header on the book detail screen.
//
//  Split out of `BookDetailView` because the header is the screen's whole
//  argument — a book is an object, not a record — and it carries enough
//  composition (accent field, halo, parallax, title block) to be read on its
//  own.

import SwiftUI

/// The accent field the jacket sits in.
///
/// The halo is centred *behind the artwork*, not at the top of the screen: a
/// gradient that peaks under the navigation bar and resolves to black by the
/// time it reaches the cover leaves the one object worth lighting unlit, which
/// is what made the old header read as a void with a picture in it.
struct BookHeroWash: View {
    let tone: OKLCH

    @Environment(\.palette) private var palette

    private var isLight: Bool { palette.bg0.l > 0.5 }

    var body: some View {
        ZStack {
            LinearGradient(
                stops: [
                    .init(color: OKLCH(isLight ? 0.92 : 0.26, tone.c * 0.7, tone.h).color, location: 0),
                    .init(color: OKLCH(isLight ? 0.95 : 0.19, tone.c * 0.5, tone.h).color, location: 0.55),
                    .init(color: palette.bg0Color, location: 0.9),
                    .init(color: palette.bg0Color, location: 1),
                ],
                startPoint: .top,
                endPoint: .bottom
            )

            RadialGradient(
                colors: [
                    OKLCH(isLight ? 0.86 : 0.52, tone.c * 0.95, tone.h).color.opacity(0.75),
                    OKLCH(isLight ? 0.90 : 0.30, tone.c * 0.6, tone.h).color.opacity(0.25),
                    .clear,
                ],
                center: UnitPoint(x: 0.5, y: 0.36),
                startRadius: 0,
                endRadius: 300
            )
        }
    }
}

/// Cover, title, byline, series, formats.
struct BookHero: View {
    let book: Book
    /// Current scroll offset, used for the parallax and rubber-band stretch.
    var scrollY: CGFloat

    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var tone: OKLCH { CoverIdentity(book).tone }

    /// Overscroll grows the jacket slightly; scrolling away lets it drift up
    /// slower than the page. Both are clamped so a fling can't distort it.
    private var stretch: CGFloat {
        guard !reduceMotion, scrollY < 0 else { return 1 }
        return 1 + min(0.12, -scrollY * 0.0009)
    }

    private var parallax: CGFloat {
        guard !reduceMotion, scrollY > 0 else { return 0 }
        return min(40, scrollY * 0.18)
    }

    /// Dissolves the jacket as it passes under the bar.
    ///
    /// Only the cover fades — fading the whole hero leaves its ~430pt of layout
    /// behind as an empty band of wash, because opacity doesn't collapse space.
    /// By the time this reaches zero the cover is mostly behind the bar anyway,
    /// so what's left is a slim strip of accent field rather than a hole.
    private var coverFade: Double {
        guard scrollY > 0 else { return 1 }
        return 1 - Double(min(1, max(0, (scrollY - 40) / 120)))
    }

    var body: some View {
        VStack(spacing: Spacing.md) {
            BookCover(identity: CoverIdentity(book), size: .lg, cornerRadius: 8)
                .frame(width: 178)
                .coverShadow(1.2)
                .scaleEffect(stretch, anchor: .bottom)
                .offset(y: parallax)
                .opacity(coverFade)
                .padding(.top, Spacing.sm)

            titleBlock
                .offset(y: parallax * 0.5)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, Spacing.xl)
    }

    private var titleBlock: some View {
        VStack(spacing: 8) {
            // The same accent lead-in the masthead uses, so a pushed screen is
            // recognisably part of the same publication.
            Rectangle()
                .fill(palette.accentColor)
                .frame(width: 26, height: 1.5)
                .padding(.bottom, 2)

            Text(book.displayTitle)
                .font(.display(33))
                .foregroundStyle(palette.ink0Color)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            authorLine

            if let series = book.series, let seriesId = book.seriesId {
                NavigationLink(value: Destination.series(id: seriesId)) {
                    Label(
                        book.seriesIndex.map { "\(series) · Book \($0)" } ?? series,
                        systemImage: "books.vertical"
                    )
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink2Color)
                }
                .padding(.top, 1)
            }

            HStack(spacing: 5) {
                ForEach(book.formats, id: \.self) { Badge(text: $0) }
                if book.hasPhysical {
                    Badge(text: "physical", tint: palette.okColor)
                }
            }
            .padding(.top, 4)
        }
        .screenPadding()
    }

    /// Authors as one centered, comma-joined line. `FlowLayout` is
    /// leading-aligned, which left-anchored the byline under a centered title.
    @ViewBuilder
    private var authorLine: some View {
        let named = Array(book.creators.prefix(3))
        if let first = named.first, let id = first.id, named.count == 1 {
            NavigationLink(value: Destination.author(id: id)) {
                Text(first.name)
                    .font(.ui(15, weight: .medium))
                    .foregroundStyle(palette.accentColor)
                    .multilineTextAlignment(.center)
            }
        } else {
            Text(named.isEmpty ? book.authorDisplay : named.map(\.name).joined(separator: " · "))
                .font(.ui(15, weight: named.isEmpty ? .regular : .medium))
                .foregroundStyle(named.isEmpty ? palette.ink2Color : palette.accentColor)
                .multilineTextAlignment(.center)
                .lineLimit(2)
        }
    }
}
