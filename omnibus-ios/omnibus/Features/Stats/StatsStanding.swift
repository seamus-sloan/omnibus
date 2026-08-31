//  StatsStanding.swift
//  The sections the period control does not govern.
//
//  Streak, the four-week strip and the activity grid lead the screen because
//  they answer "how am I doing right now"; the rest — what's open, the
//  trailing twelve months, the library's own scale — sit under the "Not tied
//  to the window" rule at the foot. None of them may move when the period
//  switcher does: `current_streak_days`, `heatmap` and `books_per_month` are
//  unwindowed on the wire, and the library figures aren't on the payload at
//  all.

import SwiftUI

// MARK: - Streak

/// The run you are on, and the record beside it.
///
/// Always "streak", never "run": the tab's own tile, the widget and the web
/// card all say streak, and one surface renaming it makes them read as two
/// different figures.
struct StreakHeadline: View {
    let summary: StatsSummary

    @Environment(\.palette) private var palette

    private var isLive: Bool { summary.currentStreakDays > 0 }

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: isLive ? "flame.fill" : "flame")
                .font(.system(size: 19))
                .foregroundStyle(isLive ? palette.accentColor : palette.ink3Color)

            Text(isLive ? "\(summary.currentStreakDays)-day streak" : "No streak going")
                .font(.display(27, weight: .semibold))
                .foregroundStyle(isLive ? palette.ink0Color : palette.ink2Color)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
                .contentTransition(.numericText())

            Spacer(minLength: Spacing.sm)

            if summary.longestStreakDays > 0 {
                Text("best \(summary.longestStreakDays)")
                    .font(.monoUI(10, weight: .bold))
                    .tracking(0.8)
                    .textCase(.uppercase)
                    .foregroundStyle(palette.ink3Color)
                    .layoutPriority(1)
            }
        }
        .screenPadding()
        .accessibilityElement(children: .combine)
    }
}

/// The last 28 days as a sparkline, with the day the current streak began.
///
/// Four weeks rather than the heatmap's half-year: this is the run's own
/// shape, and at 28 bars a rest day is still a gap you can point at.
struct LastFourWeeksCard: View {
    let summary: StatsSummary

    @Environment(\.palette) private var palette

    /// Built once per render and passed down. As a computed property this was
    /// rebuilding a dictionary over the whole (now all-time) heatmap on every
    /// read, and `sparkline` alone reads it three times.
    private var days: [DayActivity] { Self.trailing(28, of: summary) }

    private func caption(_ days: [DayActivity]) -> String {
        guard summary.currentStreakDays > 0 else {
            let active = days.filter { $0.seconds > 0 }.count
            return active == 0
                ? "Nothing logged in the last four weeks"
                : "\(active) of the last \(days.count) days"
        }
        guard let asOf = StatsFormat.wireDay.date(from: summary.asOfDay),
            let start = StatsFormat.utc.date(
                byAdding: .day, value: -(Int(summary.currentStreakDays) - 1), to: asOf)
        else { return "\(summary.currentStreakDays) days unbroken" }
        return "Unbroken since \(StatsFormat.day(start, "d MMMM"))"
    }

    var body: some View {
        let days = self.days
        return StatsCard(padding: 0) {
            HStack(alignment: .center, spacing: 14) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Last four weeks")
                        .font(.monoUI(10, weight: .bold))
                        .tracking(0.8)
                        .textCase(.uppercase)
                        .foregroundStyle(palette.ink2Color)
                    Text(caption(days))
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink1Color)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Spacer(minLength: Spacing.sm)

                sparkline(days)
                    .frame(maxWidth: 172)
            }
            .padding(.horizontal, Spacing.lg)
            .padding(.vertical, 16)
        }
        .screenPadding()
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Last four weeks. \(caption(days))")
    }

    private func sparkline(_ days: [DayActivity]) -> some View {
        let peak = max(1, days.map(\.seconds).max() ?? 1)
        let firstStreakBar = days.count - min(days.count, Int(summary.currentStreakDays))
        return HStack(alignment: .bottom, spacing: 3) {
            ForEach(Array(days.enumerated()), id: \.offset) { index, day in
                let fraction = Double(day.seconds) / Double(peak)
                RoundedRectangle(cornerRadius: 2, style: .continuous)
                    .fill(barColor(index: index, from: firstStreakBar, seconds: day.seconds))
                    .frame(maxWidth: .infinity)
                    .frame(height: max(4, fraction * 30))
            }
        }
        .frame(height: 30, alignment: .bottom)
        .accessibilityHidden(true)
    }

    private func barColor(index: Int, from firstStreakBar: Int, seconds: Int64) -> Color {
        if seconds == 0 { return palette.bg3Color }
        return index >= firstStreakBar ? palette.accentColor : StatsRamp.quiet.color
    }

    /// The last `count` days ending on the server's day, gaps filled with
    /// zeroes — the heatmap carries active days only, and a sparkline drawn
    /// off those alone would silently close its own rest days up.
    static func trailing(_ count: Int, of summary: StatsSummary) -> [DayActivity] {
        guard let asOf = StatsFormat.wireDay.date(from: summary.asOfDay) else { return [] }
        let lookup = Dictionary(summary.heatmap.map { ($0.day, $0.seconds) }, uniquingKeysWith: +)
        let calendar = StatsFormat.utc
        return (0..<count).reversed().compactMap { back in
            guard let date = calendar.date(byAdding: .day, value: -back, to: asOf) else {
                return nil
            }
            let key = StatsFormat.wireDay.string(from: date)
            return DayActivity(day: key, seconds: lookup[key] ?? 0)
        }
    }
}

// MARK: - In progress

/// What is open right now — a standing fact about the shelf, not something a
/// period switch has any business moving.
struct InProgressCard: View {
    let points: [ResumePoint]

    @Environment(\.palette) private var palette

    var body: some View {
        StatsCard {
            VStack(alignment: .leading, spacing: 14) {
                ForEach(points) { point in
                    NavigationLink(value: Destination.book(uuid: point.record.bookUUID)) {
                        row(point)
                    }
                    .buttonStyle(PressableStyle())
                }
            }
        }
    }

    private func row(_ point: ResumePoint) -> some View {
        HStack(spacing: 12) {
            BookCover(identity: CoverIdentity(point.book), size: .sm, cornerRadius: 3)
                .frame(width: 36, height: 54)

            VStack(alignment: .leading, spacing: 0) {
                Text(point.book.displayTitle)
                    .font(.display(18))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                if let author = point.book.creators.first?.name {
                    Text(author)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                        .lineLimit(1)
                        .padding(.top, 2)
                }
                StatsBar(
                    fraction: point.fraction ?? 0,
                    height: 3,
                    track: palette.bg3Color
                )
                .padding(.top, 7)
                .opacity(point.fraction == nil ? 0 : 1)
            }

            Text(Self.positionLabel(point))
                .font(.monoUI(10))
                .foregroundStyle(palette.ink2Color)
                .layoutPriority(1)
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    /// What is left, in the unit the format has. Audio can say it in hours
    /// because it knows its own length; a CFI-only reading position cannot, so
    /// it reports the percentage it does carry — the same split
    /// `ContinueHero` makes, and for the same reason.
    static func positionLabel(_ point: ResumePoint) -> String {
        if point.isAudio, let total = point.totalDurationSeconds,
            let position = point.record.audioPositionSeconds
        {
            let left = Format.atRate(max(0, total - position), rate: point.playbackRate ?? 1.0)
            return Format.humanDuration(Int64(left))
        }
        guard let fraction = point.fraction else { return "\u{2014}" }
        return "\(Int((fraction * 100).rounded()))%"
    }
}

// MARK: - Trailing twelve months

/// Books finished per month over the trailing year.
///
/// Never tied to the period switcher — the payload's own `books_per_month` is
/// a fixed trailing-12 window, so this reads the same on Week as on Lifetime.
struct TrailingYearCard: View {
    let months: [MonthCount]

    @Environment(\.palette) private var palette

    private var peak: Int64 { max(1, months.map(\.books).max() ?? 1) }

    private var average: Double {
        guard !months.isEmpty else { return 0 }
        return Double(months.reduce(0) { $0 + $1.books }) / Double(months.count)
    }

    var body: some View {
        StatsCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .firstTextBaseline) {
                    Text("Trailing 12 months")
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink1Color)
                    Spacer(minLength: Spacing.sm)
                    Text(String(format: "avg %.1f / mo", average))
                        .font(.monoUI(10.5))
                        .foregroundStyle(palette.ink3Color)
                }

                HStack(alignment: .bottom, spacing: 6) {
                    ForEach(Array(months.enumerated()), id: \.element.id) { index, month in
                        column(month, isLatest: index == months.count - 1)
                    }
                }
                .frame(height: 110)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Books finished per month over the trailing twelve months")
    }

    private func column(_ month: MonthCount, isLatest: Bool) -> some View {
        VStack(spacing: 7) {
            Spacer(minLength: 0)
            UnevenRoundedRectangle(topLeadingRadius: 3, topTrailingRadius: 3, style: .continuous)
                .fill(barColor(month, isLatest: isLatest))
                .frame(maxWidth: .infinity)
                .frame(height: max(3, Double(month.books) / Double(peak) * 86))
            Text(Self.initial(month.month))
                .font(.monoUI(9))
                .foregroundStyle(isLatest ? palette.accentColor : palette.ink3Color)
        }
    }

    private func barColor(_ month: MonthCount, isLatest: Bool) -> Color {
        if isLatest { return palette.accentColor }
        return month.books == peak ? StatsRamp.c1.color : palette.bg3Color
    }

    /// The month's initial, off the wire's `YYYY-MM`. Twelve columns at a
    /// phone's width have room for one letter and no more.
    static func initial(_ month: String) -> String {
        guard let index = Int(month.suffix(2)), (1...12).contains(index) else { return "" }
        return ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"][index - 1]
    }
}

// MARK: - Activity heatmap

/// GitHub-style trailing half-year activity grid, anchored on the server's day
/// so the client's clock never shifts the columns.
struct HeatmapView: View {
    let days: [DayActivity]
    let asOf: String

    @Environment(\.palette) private var palette

    private static let cell: CGFloat = 11
    private static let gap: CGFloat = 3

    /// Everything the grid needs, derived once.
    ///
    /// `lookup`, `maximum` and `weeks` were computed properties, and
    /// `color(for:)` reads the first two — so each of the 182 cells rebuilt the
    /// whole dictionary and re-scanned for the peak, while `weeks` was walked
    /// separately by the grid, the ruler and the labels. That is O(cells ×
    /// days) for an answer that doesn't change within a render, and this
    /// screen now hands the view the *all-time* heatmap rather than one
    /// window's.
    private struct Grid {
        let weeks: [[Date]]
        let lookup: [String: Int64]
        let maximum: Int64
    }

    private var calendar: Calendar { StatsFormat.utc }

    private func makeGrid() -> Grid {
        Grid(
            weeks: weeks,
            lookup: Dictionary(days.map { ($0.day, $0.seconds) }, uniquingKeysWith: +),
            maximum: max(1, days.map(\.seconds).max() ?? 1)
        )
    }

    private var weeks: [[Date]] {
        let end = StatsFormat.wireDay.date(from: asOf) ?? Date()
        guard let start = calendar.date(byAdding: .day, value: -181, to: end) else { return [] }

        var result: [[Date]] = []
        var current: [Date] = []
        var cursor = start
        while cursor <= end {
            current.append(cursor)
            if current.count == 7 {
                result.append(current)
                current = []
            }
            cursor = calendar.date(byAdding: .day, value: 1, to: cursor) ?? end.addingTimeInterval(1)
        }
        if !current.isEmpty { result.append(current) }
        return result
    }

    var body: some View {
        let model = makeGrid()
        return VStack(alignment: .leading, spacing: Spacing.sm) {
            ScrollView(.horizontal) {
                VStack(alignment: .leading, spacing: 5) {
                    monthRuler(model)
                    grid(model)
                }
                .padding(.vertical, 2)
                .screenPadding()
            }
            .scrollIndicators(.hidden)
            .defaultScrollAnchor(.trailing)

            legend
                .screenPadding()
        }
    }

    private func grid(_ model: Grid) -> some View {
        HStack(spacing: Self.gap) {
            ForEach(Array(model.weeks.enumerated()), id: \.offset) { _, week in
                VStack(spacing: Self.gap) {
                    ForEach(week, id: \.self) { day in
                        RoundedRectangle(cornerRadius: 2.5, style: .continuous)
                            .fill(color(for: day, in: model))
                            .frame(width: Self.cell, height: Self.cell)
                    }
                }
            }
        }
    }

    /// Month names over the column each month starts in. Without them the grid
    /// is 26 anonymous columns and a lit square means nothing.
    ///
    /// Each label is laid out in a zero-width overlay so it can overhang its
    /// own 11pt column: constrained to the column it wraps to two lines, which
    /// is how the ruler came out reading "Fe / b".
    private func monthRuler(_ model: Grid) -> some View {
        HStack(spacing: Self.gap) {
            ForEach(Array(monthLabels(model.weeks).enumerated()), id: \.offset) { _, label in
                Color.clear
                    .frame(width: Self.cell, height: 11)
                    .overlay(alignment: .leading) {
                        if let label {
                            Text(label)
                                .font(.monoUI(8.5, weight: .medium))
                                .foregroundStyle(palette.ink3Color)
                                .fixedSize()
                        }
                    }
            }
        }
    }

    /// One label per column, and only where the month actually turns over —
    /// testing each column for "contains a day in the first week" labelled two
    /// adjacent columns whenever a month started mid-week ("Ap Ap").
    private func monthLabels(_ weeks: [[Date]]) -> [String?] {
        var lastLabelled: Int?
        return weeks.enumerated().map { index, week in
            guard let first = week.first else { return nil }
            let month = calendar.component(.month, from: first)
            defer { lastLabelled = month }
            // The leading column is usually a partial week whose label would
            // sit half off the edge, and its month is labelled again a few
            // columns along anyway.
            guard index > 0, month != lastLabelled else { return nil }
            return StatsFormat.day(first, "MMM")
        }
    }

    private var legend: some View {
        HStack(spacing: 5) {
            Spacer(minLength: 0)
            Text("Less")
                .font(.ui(10))
                .foregroundStyle(palette.ink3Color)
            ForEach([0.0, 0.25, 0.5, 0.75, 1.0], id: \.self) { step in
                RoundedRectangle(cornerRadius: 2, style: .continuous)
                    .fill(step == 0 ? restColor : palette.accentColor.opacity(0.25 + step * 0.75))
                    .frame(width: 9, height: 9)
            }
            Text("More")
                .font(.ui(10))
                .foregroundStyle(palette.ink3Color)
        }
    }

    /// A day with nothing on it needs to read as an empty slot in a calendar:
    /// at full `bg2` the grid was a wall of chips, at `bg1` it vanished into
    /// the page. Between the two it reads as ruled paper.
    private var restColor: Color {
        palette.bg2Color.opacity(0.75)
    }

    private func color(for day: Date, in model: Grid) -> Color {
        let seconds = model.lookup[StatsFormat.wireDay.string(from: day)] ?? 0
        guard seconds > 0 else { return restColor }
        let intensity = min(1, Double(seconds) / Double(model.maximum))
        return palette.accentColor.opacity(0.25 + intensity * 0.75)
    }
}
