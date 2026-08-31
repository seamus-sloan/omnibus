//  StatsChrome.swift
//  The pieces every Stats section is built out of: the chart ramp, the mono
//  micro-label the sections are titled with, the card ground, and the progress
//  ring.
//
//  Kept apart from the sections themselves so the two bands — windowed and
//  standing — cannot drift onto different paddings or different greys.

import SwiftUI

/// The chart ramp, for the surfaces that need more than one colour to say what
/// they mean: the reading dial's intensity steps and the genre donut's slices.
///
/// Absolute rather than palette-derived. The palette carries one accent, and a
/// dial that stepped only through its own opacity read as a smudge; these are
/// the redesign's own tokens, matching the handoff's `chart C1 / C2 / C3`.
enum StatsRamp {
    static let c1 = OKLCH(0.62, 0.10, 200)
    static let c2 = OKLCH(0.58, 0.09, 150)
    static let c3 = OKLCH(0.55, 0.10, 20)
    /// The remainder slice, and the tick a near-silent hour draws.
    static let quiet = OKLCH(0.34, 0.012, 70)
}

/// The uppercase mono rule every section leads with.
///
/// Not `SectionLabel`, which sets a heading in Cormorant at 23pt: the redesign
/// leads its sections with a quiet key so the *figures* are the only display
/// type on the screen, and a serif heading above each one competed with them.
struct StatsSectionLabel: View {
    let title: String
    var color: Color?

    init(_ title: String, color: Color? = nil) {
        self.title = title
        self.color = color
    }

    @Environment(\.palette) private var palette

    var body: some View {
        Text(title)
            .font(.monoUI(10.5, weight: .bold))
            .tracking(0.8)
            .textCase(.uppercase)
            .foregroundStyle(color ?? palette.ink2Color)
            .accessibilityAddTraits(.isHeader)
    }
}

/// The card ground: `bg1` under a hairline border, 14pt radius, 18pt padding.
struct StatsCard<Content: View>: View {
    var padding: CGFloat = Spacing.lg
    @ViewBuilder var content: () -> Content

    @Environment(\.palette) private var palette

    var body: some View {
        content()
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .fill(palette.bg1Color)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 0.5)
            )
    }
}

/// A section: its mono label, then its content, inset to the screen margin.
struct StatsSection<Content: View>: View {
    let title: String
    var spacing: CGFloat = 10
    @ViewBuilder var content: () -> Content

    init(_ title: String, spacing: CGFloat = 10, @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.spacing = spacing
        self.content = content
    }

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            StatsSectionLabel(title)
            content()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .screenPadding()
    }
}

/// A progress arc drawn as a stroked ring rather than a filled disc with a
/// hole punched in it.
///
/// The arc **clamps at 100%** so an over-target day reads as complete rather
/// than wrapping past its own start; the ratio beside it stays honest. The
/// inset is half the stroke width, which is what puts the ring's outer edge on
/// the frame rather than half a stroke outside it.
struct GoalRing<Center: View>: View {
    let fraction: Double
    let color: Color
    /// Grown from the handoff's 74pt, which left the `of N` line short of the
    /// arc. A circle's hole is only its full width across the middle, and that
    /// line sits *below* the middle: at 74/0.74 the hole is 54.8pt wide at the
    /// centre but 45.8pt at the line's own height, against a 25.5pt "of 40".
    var diameter: CGFloat = 82
    /// The inner disc as a share of the diameter. Raised with it, so the hole
    /// gains more than the growth alone would give — 54.8pt → 62.3pt, and
    /// 45.8 → 54.6 at the line that needed it — while the stroke stays the
    /// weight the handoff drew (9.6pt → 9.8pt).
    var innerScale: CGFloat = 0.76
    @ViewBuilder var center: () -> Center

    @Environment(\.palette) private var palette

    private var lineWidth: CGFloat { diameter * (1 - innerScale) / 2 }

    var body: some View {
        ZStack {
            Circle()
                .inset(by: lineWidth / 2)
                .stroke(palette.bg3Color, lineWidth: lineWidth)
            Circle()
                .inset(by: lineWidth / 2)
                .trim(from: 0, to: min(1, max(0, fraction)))
                .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .butt))
                // Trim starts at 3 o'clock; a progress ring starts at 12.
                .rotationEffect(.degrees(-90))
                .animation(Motion.settle, value: fraction)
            center()
        }
        .frame(width: diameter, height: diameter)
    }
}

/// A capsule track with a proportional fill — the weekday strip's bar and the
/// in-progress row's rule.
struct StatsBar: View {
    let fraction: Double
    var color: Color?
    var height: CGFloat = 6
    var track: Color?

    @Environment(\.palette) private var palette

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(track ?? palette.bg2Color)
                Capsule()
                    .fill(color ?? palette.accentColor)
                    .frame(width: geometry.size.width * min(1, max(0, fraction)))
            }
        }
        .frame(height: height)
        .animation(Motion.settle, value: fraction)
    }
}

// MARK: - Formatting shared across the two bands

enum StatsFormat {
    /// The server's UTC `YYYY-MM-DD`, parsed. Fixed-format, so it is pinned to
    /// `en_US_POSIX` and Gregorian — a device on a non-Gregorian calendar
    /// would otherwise fail to read the wire format at all.
    static let wireDay: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    /// UTC Gregorian — every day arithmetic on this screen is done against the
    /// server's calendar, never the device's.
    static var utc: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? .gmt
        return calendar
    }

    /// A day rendered in a pinned English format. The copy around it ("Week of
    /// …", "Unbroken since …") is English, and the device locale would splice
    /// a translated month into it.
    static func day(_ date: Date, _ format: String) -> String {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = format
        return f.string(from: date)
    }

    /// A signed whole-number delta — "+2", "−1" — or `nil` when there is
    /// nothing to compare against. The minus is a real minus sign, not a
    /// hyphen: at Space Mono's weight the hyphen reads as a dash in a number.
    static func delta(_ current: Int64, _ previous: Int64) -> String? {
        guard current != previous else { return nil }
        let difference = current - previous
        return difference > 0 ? "+\(difference)" : "\u{2212}\(-difference)"
    }

    /// A percentage delta — "+18%". `nil` when the baseline is zero, where a
    /// percentage change is not defined and "+∞%" is not an answer.
    static func percentDelta(_ current: Int64, _ previous: Int64) -> String? {
        guard previous > 0, current != previous else { return nil }
        let change = Int(((Double(current - previous) / Double(previous)) * 100).rounded())
        guard change != 0 else { return nil }
        return change > 0 ? "+\(change)%" : "\u{2212}\(-change)%"
    }

    /// "1 page" / "12 pages" — the unit is the caller's, since the figures
    /// here are bare numbers.
    static func counted(_ n: Int64, _ unit: String) -> String {
        "\(n) \(unit)\(n == 1 ? "" : "s")"
    }
}
