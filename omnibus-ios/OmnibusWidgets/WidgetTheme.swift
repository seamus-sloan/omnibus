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

    /// The Read/Play pill — the card's one solid, saturated shape. Everything
    /// else on a hero card is a wash or a hairline, so this is what the eye
    /// lands on and the reason the card reads as something to act on.
    var pillFill: Color { OKLCH(isLight ? 0.52 : 0.82, tone.c * 1.0, tone.h).color }
    var pillInk: Color { OKLCH(isLight ? 0.99 : 0.16, tone.c * 0.06, tone.h).color }

    /// A bloom of the book's own colour behind its cover, lifting the artwork
    /// off the wash instead of letting it sit on a flat panel. Same
    /// construction as the app's `HeroCard.cardGround`.
    func bloom(diameter: CGFloat) -> some View {
        RadialGradient(
            // Far weaker on a light ground. `plusLighter` on something already
            // near white has nowhere to go but grey, so the strength the app's
            // dark hero uses reads here as a smudge rather than as a glow.
            colors: [OKLCH(0.62, tone.c * 0.9, tone.h).color.opacity(isLight ? 0.16 : 0.40), .clear],
            center: .center,
            startRadius: 0,
            // Must land inside the frame's half-width, or the frame clips the
            // gradient while it still has colour and the bloom shows a hard
            // seam.
            endRadius: diameter / 2
        )
        .frame(width: diameter, height: diameter)
        .blendMode(.plusLighter)
    }

    /// The plate a coverless book gets, so a shelf of them doesn't read as
    /// grey noise. A pared-back `GeneratedCoverPlate`: the app sets the title
    /// into the artwork, which a widget draws at 34pt wide in the large family
    /// — illegible there, and a text layout pass per render for nothing.
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
            // The artwork carries the same information the title beside it
            // does, so it announces as the book rather than as an unlabelled
            // image — matching how the in-app Continue hero labels its cover.
            .accessibilityElement()
            .accessibilityLabel(book.title)
    }

    private var image: UIImage? { WidgetArt.image(for: book) }
}

/// The one place a card decodes a book's pre-rendered cover.
///
/// Shared because the small hero draws the same bytes twice — once blurred as
/// the card's ground and once sharp on top of it — and two decodes of one file
/// on a render budget as tight as a widget's is a cost for nothing.
enum WidgetArt {
    static func image(for book: WidgetBook) -> UIImage? {
        guard let name = book.thumb,
              let url = WidgetStore.thumbURL(named: name)
        else { return nil }
        return UIImage(contentsOfFile: url.path)
    }
}

/// The book's own artwork, blown up and blurred into the card's ground.
///
/// Falls back to the tone wash when there is no art — which is not a
/// degradation so much as the same idea by a cheaper route, since the tone was
/// extracted from that cover in the first place.
struct WidgetHeroBackdrop: View {
    let book: WidgetBook
    let theme: WidgetTheme

    var body: some View {
        ZStack {
            theme.ground
            if let image = WidgetArt.image(for: book) {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFill()
                    // Blurred hard enough that the crop is no longer legible
                    // as a crop. A 2:3 cover filling a square loses a third of
                    // its height, and at any gentler radius what shows is
                    // recognisably the middle of the artwork with its title
                    // sliced off.
                    .blur(radius: 24, opaque: true)
                    // A blur this heavy is an average of the whole cover, and
                    // the average of any artwork is closer to grey than any
                    // part of it was. Pushing the chroma back up is what keeps
                    // the ground reading as *this book's* colour.
                    .saturation(1.4)
                    // Graded, not flat. A single veil strong enough to carry
                    // the title at the bottom of the card washes the top of it
                    // out too, and the top is the half with the artwork's
                    // colour in it — which is the whole reason for using the
                    // cover as a ground rather than the tone.
                    .overlay {
                        LinearGradient(
                            colors: theme.isLight
                                ? [.white.opacity(0.08), .white.opacity(0.66)]
                                : [.black.opacity(0.12), .black.opacity(0.72)],
                            startPoint: .top,
                            endPoint: .bottom
                        )
                    }
            }
        }
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
