//  WidgetLabels.swift
//  The two strings a widget card formats for itself.
//
//  Shared rather than copied into the extension for the same reason `OKLCH` is:
//  each mirrors something the app already renders — `Format.humanDuration` and
//  `Format.relative(unix:)` — and two copies of a formatter drift silently,
//  leaving the widget and the Continue hero describing one book in two
//  different ways. Living here also puts them in the app module, which is what
//  makes `omnibusTests` able to assert the two sides still agree.

import Foundation

enum WidgetLabels {
    /// Compact spoken form — "4h 12m".
    ///
    /// One function taking `Double`, deliberately not a `Double`/`Int64`
    /// overload pair: an integer literal matches both, so every call written
    /// as `duration(1800)` would be ambiguous. Seconds arrive as a `Double`
    /// here because a rate-adjusted remaining time is fractional; rounding to
    /// whole seconds first is what makes this agree with
    /// `Format.humanDuration`'s `Int64` at the boundaries rather than only
    /// away from them.
    static func duration(_ seconds: Double) -> String {
        guard seconds.isFinite, seconds > 0 else { return "0m" }
        let total = Int64(seconds.rounded())
        let hours = total / 3600
        let minutes = (total % 3600) / 60
        if hours > 0 { return minutes > 0 ? "\(hours)h \(minutes)m" : "\(hours)h" }
        if minutes > 0 { return "\(minutes)m" }
        return "\(total)s"
    }

    /// "2h ago".
    ///
    /// Deliberately not `Text(_, style: .relative)`, which re-renders on the
    /// system's clock but formats as a bare duration — a book read two hours
    /// ago reads "2 hr, 0 min", which on a card full of audiobooks looks like
    /// time *remaining*.
    static func relative(_ date: Date, relativeTo now: Date = .now) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: now)
    }
}
