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
