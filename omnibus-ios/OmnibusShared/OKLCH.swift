//  OKLCH.swift
//  Exact OKLCH → sRGB conversion so the Swift tokens can be written in the
//  same notation as `frontend/assets/atrium.css` and never drift from it.

import SwiftUI

/// A color expressed in OKLCH, matching the CSS `oklch(L C H / alpha)` form.
///
/// Lightness is `0...1`, chroma is absolute (not a percentage), and hue is in
/// degrees. Converting eagerly at init keeps `Color` construction off the
/// render path.
struct OKLCH {
    let l: Double
    let c: Double
    let h: Double
    let alpha: Double

    init(_ l: Double, _ c: Double, _ h: Double, _ alpha: Double = 1.0) {
        self.l = l
        self.c = c
        self.h = h
        self.alpha = alpha
    }

    /// Linear-light sRGB components, before gamma encoding. May fall outside
    /// `0...1` for out-of-gamut colors; `srgb` clamps.
    private var linearRGB: (Double, Double, Double) {
        let hRad = h * .pi / 180
        let a = c * cos(hRad)
        let b = c * sin(hRad)

        // OKLab → LMS (cone response), cubed to undo the nonlinearity.
        let lCone = pow(l + 0.3963377774 * a + 0.2158037573 * b, 3)
        let mCone = pow(l - 0.1055613458 * a - 0.0638541728 * b, 3)
        let sCone = pow(l - 0.0894841775 * a - 1.2914855480 * b, 3)

        return (
            4.0767416621 * lCone - 3.3077115913 * mCone + 0.2309699292 * sCone,
            -1.2684380046 * lCone + 2.6097574011 * mCone - 0.3413193965 * sCone,
            -0.0041960863 * lCone - 0.7034186147 * mCone + 1.7076147010 * sCone
        )
    }

    /// Gamma-encoded sRGB in `0...1`, clamped into gamut.
    var components: (r: Double, g: Double, b: Double) {
        let (lr, lg, lb) = linearRGB
        return (encode(lr), encode(lg), encode(lb))
    }

    private func encode(_ channel: Double) -> Double {
        let v = max(0, min(1, channel))
        return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055
    }

    var color: Color {
        let (r, g, b) = components
        return Color(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }

    var uiColor: UIColor {
        let (r, g, b) = components
        return UIColor(red: r, green: g, blue: b, alpha: alpha)
    }

    /// The same hue and chroma at a new lightness — used for pressed/hover
    /// states without re-deriving a whole token.
    func lightness(_ newL: Double) -> OKLCH {
        OKLCH(newL, c, h, alpha)
    }

    func opacity(_ newAlpha: Double) -> OKLCH {
        OKLCH(l, c, h, newAlpha)
    }
}

extension OKLCH {
    /// Parse the `accent` string the indexer stores per book. Cover extraction
    /// emits `oklch(L C H)`; hex is accepted for hand-set values.
    static func parse(cssValue raw: String) -> OKLCH? {
        let s = raw.trimmingCharacters(in: .whitespaces).lowercased()

        if s.hasPrefix("oklch(") {
            let inner = s.dropFirst("oklch(".count).dropLast()
            let parts = inner
                .replacingOccurrences(of: "/", with: " ")
                .split(whereSeparator: { $0 == " " || $0 == "," })
                .compactMap { Double($0.replacingOccurrences(of: "%", with: "")) }
            guard parts.count >= 3 else { return nil }
            let l = parts[0] > 1 ? parts[0] / 100 : parts[0]
            return OKLCH(l, parts[1], parts[2], parts.count > 3 ? parts[3] : 1)
        }

        guard s.hasPrefix("#") else { return nil }
        let hex = String(s.dropFirst())
        guard hex.count == 6, let value = UInt32(hex, radix: 16) else { return nil }
        return OKLCH.fromSRGB(
            r: Double((value >> 16) & 0xFF) / 255,
            g: Double((value >> 8) & 0xFF) / 255,
            b: Double(value & 0xFF) / 255
        )
    }

    /// Inverse of `components` — sRGB back into OKLCH, so a hex accent can be
    /// manipulated on the same lightness/chroma axes as an extracted one.
    static func fromSRGB(r: Double, g: Double, b: Double) -> OKLCH {
        func decode(_ c: Double) -> Double {
            c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        let lr = decode(r), lg = decode(g), lb = decode(b)

        let l = cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
        let m = cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
        let s = cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)

        let okL = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
        let okA = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
        let okB = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s

        let chroma = sqrt(okA * okA + okB * okB)
        var hue = atan2(okB, okA) * 180 / .pi
        if hue < 0 { hue += 360 }
        return OKLCH(okL, chroma, hue)
    }

    /// Scale chroma, for muting an accent into a background wash.
    func chroma(_ scale: Double) -> OKLCH {
        OKLCH(l, c * scale, h, alpha)
    }
}

extension Color {
    init(_ oklch: OKLCH) {
        self = oklch.color
    }

    /// Parse the accent strings the indexer stores on a book row. These come
    /// from cover extraction as CSS color values — hex or `oklch(...)`.
    init?(cssValue raw: String) {
        let s = raw.trimmingCharacters(in: .whitespaces).lowercased()
        if s.hasPrefix("#") {
            let hex = String(s.dropFirst())
            guard let value = UInt32(hex, radix: 16) else { return nil }
            switch hex.count {
            case 6:
                self = Color(
                    .sRGB,
                    red: Double((value >> 16) & 0xFF) / 255,
                    green: Double((value >> 8) & 0xFF) / 255,
                    blue: Double(value & 0xFF) / 255
                )
            case 3:
                let r = (value >> 8) & 0xF
                let g = (value >> 4) & 0xF
                let b = value & 0xF
                self = Color(
                    .sRGB,
                    red: Double(r * 17) / 255,
                    green: Double(g * 17) / 255,
                    blue: Double(b * 17) / 255
                )
            default:
                return nil
            }
            return
        }
        if s.hasPrefix("oklch(") {
            let inner = s.dropFirst("oklch(".count).dropLast()
            let parts = inner
                .replacingOccurrences(of: "/", with: " ")
                .split(whereSeparator: { $0 == " " || $0 == "," })
                .compactMap { Double($0.replacingOccurrences(of: "%", with: "")) }
            guard parts.count >= 3 else { return nil }
            // CSS allows `L` as a percentage; values above 1 are percentages.
            let l = parts[0] > 1 ? parts[0] / 100 : parts[0]
            self = OKLCH(l, parts[1], parts[2], parts.count > 3 ? parts[3] : 1).color
            return
        }
        return nil
    }
}
