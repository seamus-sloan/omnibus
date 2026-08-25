//  Theme.swift
//  The Atrium palette, ported token-for-token from `frontend/assets/atrium.css`.
//  Values stay in OKLCH so a change on either side is a direct comparison.

import CoreText
import SwiftUI
import UIKit

enum ThemeName: String, CaseIterable, Codable, Sendable {
    case atrium
    case black
    case light
    case sepia

    var label: String {
        switch self {
        case .atrium: "Atrium"
        case .black: "Pure Black"
        case .light: "Light"
        case .sepia: "Sepia"
        }
    }

    var isDark: Bool {
        self == .atrium || self == .black
    }

    var colorScheme: ColorScheme {
        isDark ? .dark : .light
    }
}

/// One resolved palette. Field names mirror the CSS custom properties so a
/// token can be traced straight back to `atrium.css`.
struct Palette: Sendable {
    let bg0: OKLCH
    let bg1: OKLCH
    let bg2: OKLCH
    let bg3: OKLCH
    let line: OKLCH
    let line2: OKLCH
    let ink0: OKLCH
    let ink1: OKLCH
    let ink2: OKLCH
    let ink3: OKLCH
    let accent: OKLCH
    let accentSoft: OKLCH
    let accentInk: OKLCH
    let coverFallbackBg: OKLCH
    let coverFallbackInk: OKLCH
    let ok: OKLCH
    let warn: OKLCH
    let bad: OKLCH
    /// Reader page ground. Kept in sync with the themes `epub-reader-glue.js`
    /// registers so the prose surface is seamless with the chrome around it.
    let readerPage: Color

    static let atrium = Palette(
        bg0: OKLCH(0.135, 0.006, 70),
        bg1: OKLCH(0.175, 0.008, 70),
        bg2: OKLCH(0.215, 0.010, 70),
        bg3: OKLCH(0.265, 0.011, 70),
        line: OKLCH(0.30, 0.010, 70, 0.65),
        line2: OKLCH(0.30, 0.010, 70, 0.30),
        ink0: OKLCH(0.97, 0.006, 80),
        ink1: OKLCH(0.82, 0.010, 75),
        ink2: OKLCH(0.62, 0.012, 70),
        ink3: OKLCH(0.48, 0.012, 70),
        accent: OKLCH(0.78, 0.13, 65),
        accentSoft: OKLCH(0.30, 0.07, 65),
        accentInk: OKLCH(0.16, 0.02, 65),
        coverFallbackBg: OKLCH(0.30, 0.004, 70),
        coverFallbackInk: OKLCH(0.92, 0.004, 70),
        ok: OKLCH(0.78, 0.13, 150),
        warn: OKLCH(0.78, 0.13, 75),
        bad: OKLCH(0.72, 0.16, 25),
        readerPage: Color(red: 0x20 / 255, green: 0x1e / 255, blue: 0x1b / 255)
    )

    static let black = Palette(
        bg0: OKLCH(0, 0, 0),
        bg1: OKLCH(0.150, 0, 0),
        bg2: OKLCH(0.190, 0, 0),
        bg3: OKLCH(0.240, 0, 0),
        line: OKLCH(0.34, 0, 0, 0.60),
        line2: OKLCH(0.34, 0, 0, 0.26),
        ink0: OKLCH(0.99, 0, 0),
        ink1: OKLCH(0.82, 0, 0),
        ink2: OKLCH(0.63, 0, 0),
        ink3: OKLCH(0.48, 0, 0),
        accent: OKLCH(0.94, 0, 0),
        accentSoft: OKLCH(0.30, 0, 0),
        accentInk: OKLCH(0.14, 0, 0),
        coverFallbackBg: OKLCH(0.26, 0, 0),
        coverFallbackInk: OKLCH(0.94, 0, 0),
        ok: OKLCH(0.78, 0.13, 150),
        warn: OKLCH(0.78, 0.13, 75),
        bad: OKLCH(0.72, 0.16, 25),
        readerPage: .black
    )

    static let light = Palette(
        bg0: OKLCH(0.985, 0.003, 75),
        bg1: OKLCH(0.965, 0.004, 75),
        bg2: OKLCH(0.940, 0.006, 75),
        bg3: OKLCH(0.910, 0.008, 75),
        line: OKLCH(0.40, 0.010, 70, 0.20),
        line2: OKLCH(0.40, 0.010, 70, 0.10),
        ink0: OKLCH(0.17, 0.006, 70),
        ink1: OKLCH(0.36, 0.008, 70),
        ink2: OKLCH(0.52, 0.010, 70),
        ink3: OKLCH(0.50, 0.010, 70),
        accent: OKLCH(0.78, 0.13, 65),
        accentSoft: OKLCH(0.30, 0.07, 65),
        // Light keeps the dark palette's 0.78 L amber accent, so it takes the
        // dark ink with it — a near-white ink on that ground is ~2:1, which is
        // what made the Continue-reading capsule unreadable. Sepia can use a
        // white ink only because it drops the accent to 0.58 L.
        accentInk: OKLCH(0.16, 0.02, 65),
        coverFallbackBg: OKLCH(0.86, 0.004, 70),
        coverFallbackInk: OKLCH(0.28, 0.006, 70),
        ok: OKLCH(0.78, 0.13, 150),
        warn: OKLCH(0.78, 0.13, 75),
        bad: OKLCH(0.72, 0.16, 25),
        readerPage: Color(red: 0xfc / 255, green: 0xfb / 255, blue: 0xfa / 255)
    )

    static let sepia = Palette(
        bg0: OKLCH(0.945, 0.022, 80),
        bg1: OKLCH(0.915, 0.026, 80),
        bg2: OKLCH(0.880, 0.030, 78),
        bg3: OKLCH(0.840, 0.034, 76),
        line: OKLCH(0.42, 0.030, 60, 0.28),
        line2: OKLCH(0.42, 0.030, 60, 0.14),
        ink0: OKLCH(0.26, 0.038, 55),
        ink1: OKLCH(0.40, 0.036, 55),
        ink2: OKLCH(0.52, 0.032, 58),
        ink3: OKLCH(0.64, 0.028, 60),
        accent: OKLCH(0.58, 0.11, 50),
        accentSoft: OKLCH(0.82, 0.05, 60),
        accentInk: OKLCH(0.98, 0.012, 80),
        coverFallbackBg: OKLCH(0.80, 0.030, 72),
        coverFallbackInk: OKLCH(0.30, 0.036, 55),
        ok: OKLCH(0.62, 0.13, 150),
        warn: OKLCH(0.62, 0.13, 75),
        bad: OKLCH(0.55, 0.16, 25),
        readerPage: Color(red: 0xed / 255, green: 0xe4 / 255, blue: 0xd0 / 255)
    )

    static func named(_ name: ThemeName) -> Palette {
        switch name {
        case .atrium: .atrium
        case .black: .black
        case .light: .light
        case .sepia: .sepia
        }
    }
}

// MARK: - Environment plumbing

private struct PaletteKey: EnvironmentKey {
    static let defaultValue = Palette.atrium
}

extension EnvironmentValues {
    var palette: Palette {
        get { self[PaletteKey.self] }
        set { self[PaletteKey.self] = newValue }
    }
}

/// Convenience accessors so call sites read `theme.ink1` rather than
/// `Color(palette.ink1)` everywhere.
extension Palette {
    var bg0Color: Color { bg0.color }
    var bg1Color: Color { bg1.color }
    var bg2Color: Color { bg2.color }
    var bg3Color: Color { bg3.color }
    var lineColor: Color { line.color }
    var line2Color: Color { line2.color }
    var ink0Color: Color { ink0.color }
    var ink1Color: Color { ink1.color }
    var ink2Color: Color { ink2.color }
    var ink3Color: Color { ink3.color }
    var accentColor: Color { accent.color }
    var accentSoftColor: Color { accentSoft.color }
    var accentInkColor: Color { accentInk.color }
    var okColor: Color { ok.color }
    var warnColor: Color { warn.color }
    var badColor: Color { bad.color }
}

// MARK: - Shape + spacing scale

enum Radius {
    static let sm: CGFloat = 6
    static let md: CGFloat = 10
    static let lg: CGFloat = 14
    static let xl: CGFloat = 22
}

enum Spacing {
    static let xs: CGFloat = 4
    static let sm: CGFloat = 8
    static let md: CGFloat = 12
    static let lg: CGFloat = 18
    static let xl: CGFloat = 24
    static let screen: CGFloat = 20
}

// MARK: - Type scale

extension Font {
    /// PostScript names of the vendored display cuts — what `Font.custom`
    /// matches on, and what `Info.plist`'s `UIAppFonts` must keep resolvable.
    enum DisplayFace {
        static let regular = "CormorantGaramond-Regular"
        static let medium = "CormorantGaramond-Medium"
        static let semibold = "CormorantGaramond-SemiBold"
        static let italic = "CormorantGaramond-Italic"

        /// The vendored cut for `weight`. Only 400/500/600 are bundled — the
        /// range the scale asks for — so anything heavier resolves to SemiBold.
        static func name(for weight: Font.Weight) -> String {
            switch weight {
            case .medium: medium
            case .semibold, .bold, .heavy, .black: semibold
            default: regular
            }
        }
    }

    /// PostScript names of the vendored UI-sans cuts.
    enum UIFace {
        static let regular = "InstrumentSans-Regular"
        static let medium = "InstrumentSans-Medium"
        static let semibold = "InstrumentSans-SemiBold"

        static func name(for weight: Font.Weight) -> String {
            switch weight {
            case .medium: medium
            case .semibold, .bold, .heavy, .black: semibold
            default: regular
            }
        }
    }

    /// PostScript names of the vendored monospace cuts. Space Mono draws 400
    /// and 700 only, so `.medium` resolves to Regular — the same match a
    /// browser makes for `font-weight: 500` against this family.
    enum MonoFace {
        static let regular = "SpaceMono-Regular"
        static let bold = "SpaceMono-Bold"

        static func name(for weight: Font.Weight) -> String {
            switch weight {
            case .semibold, .bold, .heavy, .black: bold
            default: regular
            }
        }
    }

    /// Cormorant Garamond, the display face shared with the web client.
    ///
    /// A fixed size rather than a Dynamic Type one is deliberate:
    /// `Font.custom(_:size:)` opts into Dynamic Type scaling, which
    /// `.system(size:)` never did, so the plain initialiser would silently
    /// start resizing all ~60 call sites. Supporting Dynamic Type is worth
    /// doing — but as its own change, with the layouts checked. `Font(UIFont)`
    /// below keeps that same fixed behaviour.
    ///
    /// `weight` picks a real vendored cut rather than a CoreText embolden of
    /// the regular one, so the drawing stays Cormorant's at every step.
    static func display(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        liningFigures(DisplayFace.name(for: weight), size)
    }

    /// The true italic cut. `display(_:).italic()` would skew the upright face
    /// instead, and Cormorant Garamond's italic is a separate drawing — far
    /// more than a slant — so the synthetic version reads as a different
    /// typeface. Reserved for quoted passage text: headers are upright.
    static func displayItalic(_ size: CGFloat) -> Font {
        liningFigures(DisplayFace.italic, size)
    }

    /// Instrument Sans, the UI face shared with the web client. `fixedSize:`
    /// and the real-cut weight selection carry over from `display`.
    static func ui(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .custom(UIFace.name(for: weight), fixedSize: size)
    }

    /// Space Mono — numerals, keys, and the uppercase micro-labels.
    static func monoUI(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .custom(MonoFace.name(for: weight), fixedSize: size)
    }

    /// Cormorant Garamond draws **oldstyle** figures by default: `0` sits at
    /// x-height, so "Audiobook · 0h 06m" set in it reads as "oh o6m" and the
    /// Stats hero's "0m" reads as "om". The family carries a real lining set,
    /// so ask for it rather than keep figures out of the display face.
    /// Instrument Sans and Space Mono are lining already and need none of this.
    ///
    /// Falls back to the plain fixed-size font if the name doesn't resolve —
    /// which `ThemeFontTests` exists to make impossible.
    private static func liningFigures(_ name: String, _ size: CGFloat) -> Font {
        guard let base = UIFont(name: name, size: size) else {
            return .custom(name, fixedSize: size)
        }
        let descriptor = base.fontDescriptor.addingAttributes([
            .featureSettings: [[
                UIFontDescriptor.FeatureKey.type: kNumberCaseType,
                UIFontDescriptor.FeatureKey.selector: kUpperCaseNumbersSelector,
            ]]
        ])
        return Font(UIFont(descriptor: descriptor, size: size))
    }
}
