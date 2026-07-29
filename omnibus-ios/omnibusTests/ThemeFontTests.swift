//  ThemeFontTests.swift
//  The vendored Instrument Serif faces actually resolve at runtime.
//
//  Every link in the chain fails silently: a font file missing from the bundle,
//  a `UIAppFonts` entry that doesn't match a filename, or a PostScript name
//  typo in `Font.DisplayFace` all degrade to the system face with no error —
//  the app just quietly stops looking like the web client. These tests run
//  hosted in the app, so `UIFont(name:)` sees exactly what SwiftUI's
//  `Font.custom` will.

import SwiftUI
import Testing
import UIKit

@testable import omnibus

struct ThemeFontTests {
    @Test func instrumentSerifRegularResolvesByPostScriptName() {
        #expect(UIFont(name: Font.DisplayFace.regular, size: 17) != nil)
    }

    @Test func instrumentSerifItalicResolvesByPostScriptName() {
        #expect(UIFont(name: Font.DisplayFace.italic, size: 17) != nil)
    }
}
