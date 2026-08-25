//  ThemeFontTests.swift
//  The vendored display / sans / mono faces actually resolve at runtime.
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
    /// Every weight the type scale can be asked for, so a mapping that points
    /// at an unvendored cut is caught here rather than by eye on one screen.
    private static let weights: [Font.Weight] = [
        .ultraLight, .thin, .light, .regular, .medium, .semibold, .bold, .heavy, .black,
    ]

    @Test func cormorantGaramondResolvesForEveryDisplayWeight() {
        for weight in Self.weights {
            let name = Font.DisplayFace.name(for: weight)
            #expect(UIFont(name: name, size: 17) != nil, "display weight \(weight) -> \(name)")
        }
    }

    @Test func cormorantGaramondItalicResolvesByPostScriptName() {
        #expect(UIFont(name: Font.DisplayFace.italic, size: 17) != nil)
    }

    @Test func instrumentSansResolvesForEveryUIWeight() {
        for weight in Self.weights {
            let name = Font.UIFace.name(for: weight)
            #expect(UIFont(name: name, size: 17) != nil, "ui weight \(weight) -> \(name)")
        }
    }

    @Test func spaceMonoResolvesForEveryMonoWeight() {
        for weight in Self.weights {
            let name = Font.MonoFace.name(for: weight)
            #expect(UIFont(name: name, size: 17) != nil, "mono weight \(weight) -> \(name)")
        }
    }

    /// A resolved name is not enough: `UIFont(name:)` succeeding on the wrong
    /// family would still swap the voice, so pin the family each face reports.
    @Test func vendoredFacesReportTheExpectedFamilies() {
        #expect(UIFont(name: Font.DisplayFace.regular, size: 17)?.familyName == "Cormorant Garamond")
        #expect(UIFont(name: Font.UIFace.regular, size: 17)?.familyName == "Instrument Sans")
        #expect(UIFont(name: Font.MonoFace.regular, size: 17)?.familyName == "Space Mono")
    }

    /// A non-regular weight must land on its own drawing, not a synthetic
    /// embolden of the regular cut.
    @Test func nonRegularWeightsSelectDistinctCuts() {
        #expect(Font.DisplayFace.name(for: .semibold) != Font.DisplayFace.regular)
        #expect(Font.UIFace.name(for: .medium) != Font.UIFace.regular)
        #expect(Font.MonoFace.name(for: .bold) != Font.MonoFace.regular)
    }

    /// Cormorant Garamond's default figures are oldstyle — a `0` at x-height
    /// that reads as a lowercase `o` in "0h 06m". `Font.display` asks for the
    /// lining set; assert the family actually carries one, since a missing
    /// feature degrades silently back to oldstyle.
    @Test func cormorantGaramondCarriesALiningFigureSet() throws {
        let font = try #require(UIFont(name: Font.DisplayFace.regular, size: 17))
        let features = CTFontCopyFeatures(font as CTFont) as? [[String: Any]] ?? []
        let numberCase = features.first {
            $0[kCTFontFeatureTypeIdentifierKey as String] as? Int == kNumberCaseType
        }
        let selectors = numberCase?[kCTFontFeatureTypeSelectorsKey as String] as? [[String: Any]] ?? []
        #expect(
            selectors.contains {
                $0[kCTFontFeatureSelectorIdentifierKey as String] as? Int == kUpperCaseNumbersSelector
            }
        )
    }
}
