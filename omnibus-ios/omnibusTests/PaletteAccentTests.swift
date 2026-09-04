//  PaletteAccentTests.swift
//  The book-toned accent derivation behind the detail screen: the tone's hue
//  carries the identity, the theme's lightness band keeps the ink contract.

import Testing

@testable import omnibus

@Test func accentedPaletteTakesTheTonesHue() {
    let toned = Palette.atrium.accented(by: OKLCH(0.52, 0.16, 28))
    #expect(toned.accent.h == 28)
    #expect(toned.accentSoft.h == 28)
}

@Test func accentedPaletteKeepsTheThemesLightnessForInkContrast() {
    let toned = Palette.sepia.accented(by: OKLCH(0.95, 0.02, 200))
    #expect(toned.accent.l == Palette.sepia.accent.l)
    #expect(toned.accentSoft.l == Palette.sepia.accentSoft.l)
    #expect(toned.accentInk.l == Palette.sepia.accentInk.l)
}

@Test func accentedPaletteClampsChromaIntoTheAccentBand() {
    // A neon tone is tempered; a grey one keeps a trace of identity.
    let neon = Palette.atrium.accented(by: OKLCH(0.6, 0.4, 140))
    #expect(neon.accent.c == 0.16)
    let grey = Palette.atrium.accented(by: OKLCH(0.3, 0.0, 0))
    #expect(grey.accent.c == 0.05)
}

@Test func accentedPaletteLeavesEverythingElseAlone() {
    let toned = Palette.atrium.accented(by: OKLCH(0.52, 0.16, 28))
    #expect(toned.bg0.l == Palette.atrium.bg0.l)
    #expect(toned.ink0.l == Palette.atrium.ink0.l)
    #expect(toned.ok.h == Palette.atrium.ok.h)
    #expect(toned.bad.h == Palette.atrium.bad.h)
}

// MARK: - Toning a whole book

/// `accented(byCoverOf:)` is what the detail page and the player both run on,
/// so these pin that it resolves the tone through `CoverIdentity` — the same
/// source the cover plate and the player's backdrop bloom use — and that it
/// goes through `accented(by:)` rather than taking a cover colour raw.
private func book(accent: String?, title: String = "Piranesi") -> Book {
    var book = Book(id: 1, filename: "b.m4b")
    book.title = title
    book.accent = accent
    return book
}

@Test func bookTonedPaletteResolvesThroughTheCoverIdentitysTone() {
    let subject = book(accent: "oklch(0.62 0.13 265)")
    let toned = Palette.atrium.accented(byCoverOf: subject)
    let expected = Palette.atrium.accented(by: CoverIdentity(subject).tone)
    #expect(toned.accent.h == expected.accent.h)
    #expect(toned.accent.c == expected.accent.c)
    #expect(toned.accent.l == expected.accent.l)
}

@Test func bookTonedPaletteKeepsTheInkContractForEveryCover() {
    // A book with no cover art at all, a near-black jacket, and a neon one.
    // The tone reaches the accent in each case, but never the lightness the
    // theme's `accentInk` was chosen against — so the label still reads.
    for subject in [
        book(accent: nil),
        book(accent: "oklch(0.09 0.01 260)"),
        book(accent: "oklch(0.78 0.37 140)"),
    ] {
        let toned = Palette.atrium.accented(byCoverOf: subject)
        #expect(toned.accent.l == Palette.atrium.accent.l)
        #expect(toned.accent.c >= 0.05)
        #expect(toned.accent.c <= 0.16)
    }
}

@Test func bookTonedPaletteGivesACoverlessBookItsTitleHue() {
    // No accent to parse, so the hue comes off the title — and two different
    // coverless books are still told apart by their controls.
    let piranesi = Palette.atrium.accented(byCoverOf: book(accent: nil))
    let babel = Palette.atrium.accented(byCoverOf: book(accent: nil, title: "Babel"))
    #expect(piranesi.accent.h != babel.accent.h)
}
