//  WidgetTheme.swift
//  The card's colours, all derived from the book's own tone.
//
//  Deliberately not a copy of the app's `Palette`: the reader's chosen theme
//  lives in the app's `UserDefaults`, which the extension cannot read, and a
//  duplicated token table would be a second place for the two to disagree.
//  The Continue hero already takes its colour from the book rather than from
//  a global accent, so a widget built the same way needs no palette at all.

import SwiftUI

struct WidgetTheme {
    let tone: OKLCH
    let isLight: Bool

    init(tone: WidgetBook.Tone?, scheme: ColorScheme) {
        self.tone = tone.map { OKLCH($0.l, $0.c, $0.h) } ?? OKLCH(0.55, 0.02, 250)
        isLight = scheme == .light
    }

    /// The card ground: the book's tone lifted to a wash, graded corner to
    /// corner. Same construction as `HeroCard.cardGround`.
    var ground: LinearGradient {
        let wash = OKLCH(isLight ? 0.93 : 0.24, tone.c * 0.6, tone.h)
        return LinearGradient(
            colors: [wash.color, OKLCH(wash.l * 0.82, wash.c * 0.9, tone.h).color],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }

    /// Titles and any text that has to be read at a glance.
    var ink0: Color { OKLCH(isLight ? 0.22 : 0.97, tone.c * 0.08, tone.h).color }
    /// Authors, position readouts.
    var ink1: Color { OKLCH(isLight ? 0.40 : 0.80, tone.c * 0.10, tone.h).color }
    /// Timestamps and the quietest labels.
    var ink2: Color { OKLCH(isLight ? 0.52 : 0.63, tone.c * 0.08, tone.h).color }

    /// The progress bar and the eyebrow — the one place the book's colour is
    /// allowed to be loud. Darker on a light ground, or it disappears into it.
    var rule: Color { OKLCH(isLight ? 0.55 : 0.80, tone.c * 0.95, tone.h).color }

    var track: Color { ink2.opacity(0.28) }

    /// The plate a coverless book gets, so a shelf of them doesn't read as
    /// grey noise. A pared-back `GeneratedCoverPlate` — a widget draws these
    /// at 34pt wide, where the app's title-in-the-artwork treatment is
    /// unreadable and only costs a text layout pass per render.
    func plate(_ bookTone: WidgetBook.Tone) -> LinearGradient {
        let base = OKLCH(bookTone.l, bookTone.c, bookTone.h)
        return LinearGradient(
            colors: [
                OKLCH(isLight ? 0.62 : 0.42, base.c * 0.85, base.h).color,
                OKLCH(isLight ? 0.48 : 0.26, base.c * 0.65, base.h).color,
            ],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }
}

/// A book's pre-rendered cover, or its plate when there isn't one.
///
/// The bytes are already in the App Group — the app puts them there at
/// snapshot time — so this is a file read and a decode, never a fetch. The
/// plate sits *under* the image rather than standing in until it loads, since
/// some covers are transparent PNGs and would otherwise render as a hole.
struct WidgetCover: View {
    let book: WidgetBook
    let theme: WidgetTheme
    var cornerRadius: CGFloat = 4

    var body: some View {
        Color.clear
            .aspectRatio(2.0 / 3.0, contentMode: .fit)
            .overlay { theme.plate(book.tone) }
            .overlay {
                if let image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFill()
                }
            }
            // An aspect-fill image reports a size larger than the box it
            // fills, so it has to be clipped inside a container that can't
            // grow — otherwise it stretches the row it sits in.
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
    }

    private var image: UIImage? {
        guard let name = book.thumb,
              let url = WidgetStore.thumbURL(named: name)
        else { return nil }
        return UIImage(contentsOfFile: url.path)
    }
}

/// The position bar. A capsule rather than the app's `ProgressBar` so the
/// widget owns its own hairline weight at three very different card sizes.
struct WidgetProgressBar: View {
    let fraction: Double
    let theme: WidgetTheme
    var height: CGFloat = 3

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(theme.track)
                Capsule()
                    .fill(theme.rule)
                    .frame(width: max(height, geometry.size.width * min(1, max(0, fraction))))
            }
        }
        .frame(height: height)
    }
}
