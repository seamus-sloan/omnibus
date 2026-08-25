//  StatsView.swift
//  Reading stats: a headline panel, supporting tiles, an activity heatmap,
//  rankings, and the finished-books rail.

import Charts
import SwiftUI

struct StatsView: View {
    @Environment(\.palette) private var palette

    @State private var range: StatsRange = .month
    @State private var summary: StatsSummary?
    @State private var isLoading = true
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Group {
                if isLoading && summary == nil {
                    LoadingView()
                } else if let error, summary == nil {
                    ErrorStateView(message: error) { Task { await load() } }
                } else if let summary {
                    content(summary)
                }
            }
            .background(ScreenBackground())
            // The masthead carries the screen's name, as on every other tab
            // root; a stock large title alongside it would state it twice.
            .toolbar(.hidden, for: .navigationBar)
            .topEdgeScrim()
            .refreshTask { await load(force: true) }
            .withDestinations()
        }
        .task { await load() }
        .onChange(of: range) { _, _ in Task { await load() } }
    }

    private func content(_ summary: StatsSummary) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 30) {
                Masthead(title: "Stats") { rangeMenu }

                headline(summary)
                tiles(summary)

                if !summary.heatmap.isEmpty {
                    section("Activity") {
                        HeatmapView(days: summary.heatmap, asOf: summary.asOfDay)
                    }
                }

                if !summary.booksPerMonth.isEmpty {
                    section("Books finished") {
                        Chart(summary.booksPerMonth) { point in
                            BarMark(
                                x: .value("Month", point.month),
                                y: .value("Books", point.books)
                            )
                            .foregroundStyle(palette.accentColor)
                            .cornerRadius(3)
                        }
                        .chartXAxis {
                            AxisMarks { value in
                                AxisValueLabel {
                                    if let month = value.as(String.self) {
                                        Text(String(month.suffix(2)))
                                            .font(.monoUI(9))
                                    }
                                }
                            }
                        }
                        .chartYAxis {
                            AxisMarks(position: .leading) { _ in
                                AxisGridLine().foregroundStyle(palette.line2.color)
                                AxisValueLabel().font(.monoUI(9))
                            }
                        }
                        .frame(height: 132)
                    }
                }

                if !summary.topAuthors.isEmpty {
                    section("Top authors") {
                        RankedList(entries: summary.topAuthors) { entry in
                            .searchResults(query: entry.name)
                        }
                    }
                }

                if !summary.topTags.isEmpty {
                    section("Top tags") {
                        RankedList(entries: summary.topTags) { entry in
                            .tag(name: entry.name)
                        }
                    }
                }

                if !summary.finishedBooks.isEmpty {
                    finishedRail(summary.finishedBooks)
                }
            }
            .padding(.bottom, 40)
        }
        .scrollIndicators(.hidden)
    }

    /// An explicit `Menu` rather than a `Picker`: a toolbar picker stretches to
    /// fill the empty bar next to a title.
    private var rangeMenu: some View {
        Menu {
            ForEach(StatsRange.allCases, id: \.self) { option in
                Button {
                    range = option
                } label: {
                    if range == option {
                        Label(option.label, systemImage: "checkmark")
                    } else {
                        Text(option.label)
                    }
                }
            }
        } label: {
            HStack(spacing: 5) {
                Text(range.label)
                    .font(.ui(12.5, weight: .medium))
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .bold))
            }
            .foregroundStyle(palette.ink1Color)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(Capsule().fill(palette.bg2Color))
            .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
        }
        .accessibilityLabel("Time range")
    }

    // MARK: - Numbers

    /// One number leads, because there is one number anybody opens this screen
    /// for. A flat grid of six equal tiles made the reader do the ranking.
    private func headline(_ summary: StatsSummary) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(range.label.uppercased())
                .font(.monoUI(10, weight: .medium))
                .tracking(0.8)
                .foregroundStyle(palette.accentColor)

            Text(Format.humanDuration(summary.totalSeconds))
                .font(.display(52))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(1)
                .minimumScaleFactor(0.5)
                .contentTransition(.numericText())

            Text(splitLabel(summary))
                .font(.ui(12.5))
                .foregroundStyle(palette.ink2Color)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Spacing.lg)
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .fill(palette.bg1Color)
        )
        .overlay(alignment: .bottom) {
            // The split, drawn: reading against listening along the foot of the
            // panel, so the ratio is visible without reading the caption. Only
            // when there are two things to split — a full-width bar stating
            // "100% reading" is a decoration that looks like information.
            if summary.readingSeconds > 0, summary.listeningSeconds > 0 {
                SplitBar(
                    reading: summary.readingSeconds,
                    listening: summary.listeningSeconds
                )
                .clipShape(
                    UnevenRoundedRectangle(
                        bottomLeadingRadius: Radius.lg,
                        bottomTrailingRadius: Radius.lg,
                        style: .continuous
                    )
                )
            }
        }
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(palette.line2.color, lineWidth: 0.5)
        )
        .animation(Motion.settle, value: summary.totalSeconds)
        .screenPadding()
    }

    /// The headline already states the total, so the caption breaks it down
    /// only when there is a breakdown — otherwise it restated the same number
    /// one line below itself ("11m" over "11m reading").
    private func splitLabel(_ summary: StatsSummary) -> String {
        guard summary.totalSeconds > 0 else { return "Nothing logged in this range yet" }

        var parts: [String] = []
        if summary.readingSeconds > 0, summary.listeningSeconds > 0 {
            parts.append("\(Format.humanDuration(summary.readingSeconds)) reading")
            parts.append("\(Format.humanDuration(summary.listeningSeconds)) listening")
        } else {
            parts.append(summary.listeningSeconds > 0 ? "Listening" : "Reading")
        }
        if summary.sessions > 0 {
            parts.append("\(summary.sessions) session\(summary.sessions == 1 ? "" : "s")")
        }
        if summary.activeDays > 0 {
            parts.append("\(summary.activeDays) day\(summary.activeDays == 1 ? "" : "s")")
        }
        return parts.joined(separator: " · ")
    }

    private func tiles(_ summary: StatsSummary) -> some View {
        LazyVGrid(
            columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)],
            spacing: 12
        ) {
            StatTile(
                label: "Books finished",
                value: "\(summary.booksFinished)",
                icon: "checkmark.circle"
            )
            StatTile(
                label: "Longest streak",
                value: summary.longestStreakDays > 0 ? "\(summary.longestStreakDays)d" : "—",
                icon: "flame"
            )
            StatTile(
                label: "Days active",
                value: summary.activeDays > 0 ? "\(summary.activeDays)" : "—",
                icon: "calendar"
            )
            StatTile(
                label: "Pages",
                value: summary.pagesRead.map { "\($0)" } ?? "—",
                icon: "doc.text"
            )
            StatTile(
                label: "Avg rating",
                value: summary.avgStars.map { String(format: "%.1f", $0) } ?? "—",
                icon: "star"
            )
            StatTile(
                label: "Books open",
                value: summary.booksActive > 0 ? "\(summary.booksActive)" : "—",
                icon: "book"
            )
        }
        .screenPadding()
    }

    private func finishedRail(_ books: [FinishedBook]) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Finished")
                .screenPadding()

            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 14) {
                    ForEach(books) { finished in
                        NavigationLink(value: Destination.book(uuid: finished.bookUUID)) {
                            VStack(alignment: .leading, spacing: 7) {
                                BookCover(
                                    identity: CoverIdentity(
                                        uuid: finished.bookUUID,
                                        title: finished.title,
                                        author: finished.author,
                                        hasCover: finished.coverURL != nil
                                    )
                                )
                                .coverShadow()

                                Text(finished.title)
                                    .font(.ui(12, weight: .medium))
                                    .foregroundStyle(palette.ink0Color)
                                    .lineLimit(2)
                                    .multilineTextAlignment(.leading)

                                if let rating = finished.rating {
                                    StarRating(stars: rating, size: 9)
                                }
                            }
                            .frame(width: 88)
                        }
                        .buttonStyle(BookPressStyle())
                    }
                }
                .screenPadding()
                .scrollTargetLayout()
            }
            .scrollIndicators(.hidden)
            .scrollTargetBehavior(.viewAligned)
        }
    }

    private func section<Content: View>(
        _ title: String, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel(title)
            content()
        }
        .screenPadding()
    }

    private func load(force: Bool = false) async {
        if force { await OfflineStore.shared.cacheDelete(CacheKey.stats(range)) }
        do {
            for try await read in UserDataService.stats(range: range) {
                summary = read.value
                error = nil
                isLoading = false
            }
        } catch {
            if summary == nil {
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
        isLoading = false
    }
}

/// Reading against listening as one two-tone rule.
private struct SplitBar: View {
    let reading: Int64
    let listening: Int64

    @Environment(\.palette) private var palette

    private var total: Double { Double(max(1, reading + listening)) }

    var body: some View {
        GeometryReader { geometry in
            HStack(spacing: 0) {
                Rectangle()
                    .fill(palette.accentColor)
                    .frame(width: geometry.size.width * Double(reading) / total)
                Rectangle()
                    .fill(palette.accentColor.opacity(0.4))
                    .frame(width: geometry.size.width * Double(listening) / total)
                Spacer(minLength: 0)
            }
        }
        .frame(height: 3)
        .frame(maxHeight: .infinity, alignment: .bottom)
        .opacity(reading + listening > 0 ? 1 : 0)
        .accessibilityHidden(true)
    }
}

private struct StatTile: View {
    let label: String
    let value: String
    let icon: String

    @Environment(\.palette) private var palette

    private var isEmpty: Bool { value == "—" }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(isEmpty ? palette.ink3Color : palette.accentColor)
            // A serif em dash at 25pt draws a 25pt rule, which reads as a
            // divider rather than as "no value". The mono face keeps the
            // placeholder the size of a character.
            Text(value)
                .font(isEmpty ? .monoUI(22) : .display(28))
                .foregroundStyle(isEmpty ? palette.ink3Color : palette.ink0Color)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
                .contentTransition(.numericText())
            Text(label)
                .font(.ui(11.5))
                .foregroundStyle(palette.ink3Color)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: Radius.md, style: .continuous).fill(palette.bg1Color)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                .strokeBorder(palette.line2.color, lineWidth: 0.5)
        )
        .accessibilityElement(children: .combine)
    }
}

private struct RankedList: View {
    let entries: [RankedEntity]
    /// Where a row leads. A ranking you can't act on is a picture of a ranking.
    let destination: (RankedEntity) -> Destination

    @Environment(\.palette) private var palette

    private var maximum: Int64 {
        max(1, entries.map(\.seconds).max() ?? 1)
    }

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(entries.prefix(6).enumerated()), id: \.element.id) { index, entry in
                NavigationLink(value: destination(entry)) {
                    row(entry, isFirst: index == 0)
                }
                .buttonStyle(PressableStyle())
            }
        }
    }

    private func row(_ entry: RankedEntity, isFirst: Bool) -> some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                Text(entry.name)
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink1Color)
                    .lineLimit(1)
                    .frame(width: 128, alignment: .leading)

                GeometryReader { geometry in
                    ZStack(alignment: .leading) {
                        Capsule()
                            .fill(palette.line2.color)
                            .frame(height: 7)
                        Capsule()
                            .fill(
                                LinearGradient(
                                    colors: [
                                        palette.accentColor.opacity(0.65),
                                        palette.accentColor,
                                    ],
                                    startPoint: .leading,
                                    endPoint: .trailing
                                )
                            )
                            .frame(
                                width: max(7, geometry.size.width * CGFloat(entry.seconds) / CGFloat(maximum)),
                                height: 7
                            )
                    }
                    .frame(maxHeight: .infinity, alignment: .center)
                }
                .frame(height: 14)

                Text(Format.humanDuration(entry.seconds))
                    .font(.monoUI(10.5))
                    .foregroundStyle(palette.ink3Color)
                    .frame(width: 52, alignment: .trailing)
            }
            .padding(.vertical, 9)
            .contentShape(Rectangle())
        }
    }
}

/// GitHub-style trailing half-year activity grid, anchored on the server's day
/// so the client's clock never shifts the columns.
private struct HeatmapView: View {
    let days: [DayActivity]
    let asOf: String

    @Environment(\.palette) private var palette

    private static let cell: CGFloat = 11
    private static let gap: CGFloat = 3

    private var lookup: [String: Int64] {
        Dictionary(days.map { ($0.day, $0.seconds) }, uniquingKeysWith: +)
    }

    private var maximum: Int64 { max(1, days.map(\.seconds).max() ?? 1) }

    private var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? .gmt
        return calendar
    }

    private static func dayFormatter() -> DateFormatter {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd"
        formatter.timeZone = TimeZone(identifier: "UTC")
        return formatter
    }

    private var weeks: [[Date]] {
        let formatter = Self.dayFormatter()
        let end = formatter.date(from: asOf) ?? Date()
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
        VStack(alignment: .leading, spacing: Spacing.sm) {
            ScrollView(.horizontal) {
                VStack(alignment: .leading, spacing: 5) {
                    monthRuler
                    grid
                }
                .padding(.vertical, 2)
            }
            .scrollIndicators(.hidden)
            .defaultScrollAnchor(.trailing)

            legend
        }
    }

    private var grid: some View {
        HStack(spacing: Self.gap) {
            ForEach(Array(weeks.enumerated()), id: \.offset) { _, week in
                VStack(spacing: Self.gap) {
                    ForEach(week, id: \.self) { day in
                        RoundedRectangle(cornerRadius: 2.5, style: .continuous)
                            .fill(color(for: day))
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
    private var monthRuler: some View {
        HStack(spacing: Self.gap) {
            ForEach(Array(monthLabels.enumerated()), id: \.offset) { _, label in
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
    private var monthLabels: [String?] {
        let formatter = DateFormatter()
        formatter.dateFormat = "MMM"
        formatter.timeZone = TimeZone(identifier: "UTC")

        var lastLabelled: Int?
        return weeks.enumerated().map { index, week in
            guard let first = week.first else { return nil }
            let month = calendar.component(.month, from: first)
            defer { lastLabelled = month }
            // The leading column is usually a partial week whose label would
            // sit half off the edge, and its month is labelled again a few
            // columns along anyway.
            guard index > 0, month != lastLabelled else { return nil }
            return formatter.string(from: first)
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

    private func color(for day: Date) -> Color {
        let seconds = lookup[Self.dayFormatter().string(from: day)] ?? 0
        guard seconds > 0 else { return restColor }
        let intensity = min(1, Double(seconds) / Double(maximum))
        return palette.accentColor.opacity(0.25 + intensity * 0.75)
    }
}
