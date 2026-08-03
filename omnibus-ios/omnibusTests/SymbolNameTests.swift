//  SymbolNameTests.swift
//  That the SF Symbols the chrome names actually exist.
//
//  `Image(systemName:)` is not checked at compile time and does not warn: a
//  misspelt or unavailable symbol draws nothing at all, so the failure ships as
//  a blank control that still takes taps.

import UIKit
import Testing

@testable import omnibus

@Suite("Named SF Symbols")
struct SymbolNameTests {
    @Test("the library's add-books glyph resolves")
    func addGlyphResolves() {
        // Carries the sheet's two actions in one mark — viewfinder brackets for
        // the barcode scanner, a plus for the upload.
        #expect(UIImage(systemName: LibraryView.addGlyph) != nil)
    }

    @Test("the book long-press menu's glyphs resolve")
    func bookMenuGlyphsResolve() {
        #expect(UIImage(systemName: BookMenuGlyph.detail) != nil)
        #expect(UIImage(systemName: BookMenuGlyph.edit) != nil)
    }
}
