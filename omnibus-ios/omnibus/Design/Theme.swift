//  Theme.swift
//  The Atrium palette, ported token-for-token from `frontend/assets/atrium.css`.
//  Values stay in OKLCH so a change on either side is a direct comparison.

import SwiftUI

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
        accentInk: OKLCH(0.99, 0, 0),
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
    /// Instrument Serif is the web display face; iOS ships New York, which is
    /// the same editorial register and avoids bundling a webfont.
    static func display(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    static func ui(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .default)
    }

    static func monoUI(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
}
