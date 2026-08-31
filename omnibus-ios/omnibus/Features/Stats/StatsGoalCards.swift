//  StatsGoalCards.swift
//  The two standing goal cards: today's daily targets, and the year's.
//
//  Both are unwindowed by design — a daily target recurs and an annual goal is
//  annual — so they sit above the period control and never move when it does.
//  Both are tappable in whole, with a chevron rather than an Edit button: the
//  card *is* the control, and a button beside a ring made the ring look like a
//  read-only figure.

import SwiftUI

// MARK: - Daily goals

/// Today's pages and minutes, each as a ring when a target exists and a bare
/// figure when it does not.
///
/// A ring is a claim about a target, so a kind with no target gets no ring and
/// no "of N" — just today's figure, centred in the same 74pt slot so a mixed
/// card still aligns.
struct DailyGoalsCard: View {
    let summary: StatsSummary
    let onEdit: () -> Void

    @Environment(\.palette) private var palette

    /// `Today` while neither kind has a target: with nothing set the card is
    /// not reporting on goals at all, so it stops claiming to.
    private var heading: String {
        summary.dailyGoals.isEmpty ? "Today" : "Daily goals"
    }

    var body: some View {
        Button(action: onEdit) {
            StatsCard {
                VStack(alignment: .leading, spacing: Spacing.lg) {
                    HStack(spacing: 7) {
                        Text(heading)
                            .font(.monoUI(10, weight: .bold))
                            .tracking(0.8)
                            .textCase(.uppercase)
                            .foregroundStyle(palette.ink2Color)
                        Spacer(minLength: Spacing.sm)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(palette.ink3Color)
                    }

                    HStack(alignment: .center, spacing: 14) {
                        ForEach(DailyGoalKind.allCases) { kind in
                            DailyGoalRow(
                                kind: kind,
                                goal: summary.dailyGoals[kind],
                                todaysFigure: DailyGoalsCard.todaysFigure(kind, summary),
                                bothUnset: summary.dailyGoals.isEmpty
                            )
                        }
                    }
                }
            }
        }
        .buttonStyle(PressableStyle())
        .screenPadding()
        .accessibilityHint("Edit your daily goals")
    }

    /// Today's figure for a kind whose target is unset.
    ///
    /// `dailyGoals.<kind>.current` is target-gated server-side — absent
    /// precisely when there is no goal — so the no-target figure has to come
    /// from the series the summary already carries. Pages are exact: the same
    /// UTC-bucketed ledger a pages goal is measured against. Minutes are
    /// **not**: the heatmap is UTC and folds in the seconds a minutes goal
    /// discloses separately as unzoned, so this figure and the one the goal
    /// reports once set can differ by up to the reader's offset plus whatever
    /// today's unplaceable sessions hold.
    static func todaysFigure(_ kind: DailyGoalKind, _ summary: StatsSummary) -> Int64 {
        let today = summary.asOfDay
        guard !today.isEmpty else { return 0 }
        switch kind {
        case .pages:
            let pages = summary.pagesDetail.daily.first { $0.label == today }?.value ?? 0
            return Int64(pages.rounded())
        case .minutes:
            let seconds = summary.heatmap.first { $0.day == today }?.seconds ?? 0
            return seconds / 60
        }
    }
}

/// One kind's slot: ring-or-figure, then the label and its note.
private struct DailyGoalRow: View {
    let kind: DailyGoalKind
    let goal: DailyGoal?
    let todaysFigure: Int64
    let bothUnset: Bool

    @Environment(\.palette) private var palette

    private var current: Int64 { goal?.current ?? todaysFigure }
    private var isMet: Bool { goal?.isMet ?? false }
    private var arcColor: Color { isMet ? palette.okColor : palette.accentColor }

    /// The bounds allow four digits (2,000 pages, 1,440 minutes), so the
    /// figure steps down rather than crowding the ring's 54.8pt inner hole —
    /// "1284" measures 40.3pt at the base size, and three digits already look
    /// cramped without the step.
    private var ringFontSize: CGFloat {
        switch String(current).count {
        case 4...: 20
        case 3: 22
        default: 24
        }
    }

    private var bareFontSize: CGFloat {
        switch String(current).count {
        case 4...: 30
        case 3: 34
        default: 38
        }
    }

    /// "today" only earns its line when the card is headed "Daily goals" and
    /// this row is the one without one — under a "Today" heading it would say
    /// the same word twice.
    private var note: String {
        guard let goal else { return bothUnset ? "" : "today" }
        if goal.isMet { return goal.over > 0 ? "\(goal.over) over" : "Done" }
        return "\(goal.remaining) to go"
    }

    var body: some View {
        HStack(spacing: 13) {
            if let goal {
                GoalRing(fraction: goal.fraction, color: arcColor) {
                    VStack(spacing: 3) {
                        Text("\(current)")
                            .font(.display(ringFontSize, weight: .semibold))
                            .foregroundStyle(palette.ink0Color)
                            .contentTransition(.numericText())
                        Text("of \(goal.target)")
                            .font(.monoUI(8.5))
                            .foregroundStyle(palette.ink3Color)
                    }
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                    .padding(.horizontal, 4)
                }
            } else {
                Text("\(current)")
                    .font(.display(bareFontSize, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                    .frame(width: 74, height: 74)
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(kind.shortLabel)
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink1Color)
                if !note.isEmpty {
                    Text(note)
                        .font(.ui(11.5))
                        .foregroundStyle(isMet ? palette.okColor : palette.ink2Color)
                }
            }
            .lineLimit(1)
            .minimumScaleFactor(0.8)

            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            goal == nil
                ? "\(kind.shortLabel) today, \(current), no goal set"
                : "\(kind.shortLabel), \(current) of \(goal?.target ?? 0)"
        )
    }
}

// MARK: - Year goal

/// The annual goal as a cumulative curve rather than a ring.
///
/// A ring answers "how far along am I" and nothing else. The curve answers the
/// question a reader actually brings to an annual target in August: at this
/// pace, do I get there — and it carries the pace, the projection and the rate
/// still needed underneath it.
struct YearGoalCard: View {
    let summary: StatsSummary
    let year: String
    let onEdit: () -> Void

    @Environment(\.palette) private var palette

    private var projection: YearProjection { YearProjection(summary: summary, year: year) }

    var body: some View {
        let p = projection
        Button(action: onEdit) {
            StatsCard {
                VStack(alignment: .leading, spacing: 0) {
                    header(p)
                    figure(p)
                    YearGoalChart(projection: p)
                        .padding(.top, 14)
                    footer(p)
                }
            }
        }
        .buttonStyle(PressableStyle())
        .screenPadding()
        .accessibilityHint("Edit your reading goal for \(year)")
    }

    private func header(_ p: YearProjection) -> some View {
        HStack(spacing: 7) {
            Text(p.target == nil ? "Books finished" : "\(year) reading goal")
                .font(.monoUI(10, weight: .bold))
                .tracking(0.8)
                .textCase(.uppercase)
                .foregroundStyle(palette.ink2Color)
                .lineLimit(1)
            Spacer(minLength: Spacing.sm)
            if p.target == nil {
                Text("\(year) so far")
                    .font(.monoUI(10))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }
            Image(systemName: "chevron.right")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(palette.ink3Color)
        }
    }

    private func figure(_ p: YearProjection) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("\(p.finished)")
                .font(.display(38, weight: .semibold))
                .foregroundStyle(palette.ink0Color)
                .contentTransition(.numericText())
            Text(
                p.target.map { "of \(StatsFormat.counted($0, "book"))" }
                    ?? (p.finished == 1 ? "book" : "books")
            )
            .font(.ui(13.5))
            .foregroundStyle(palette.ink1Color)
        }
        .padding(.top, 10)
        .lineLimit(1)
        .minimumScaleFactor(0.7)
    }

    /// Two stats, or three once a target exists. Pace is expressed as **days
    /// per book**: at any realistic rate books-per-day is an unreadable
    /// decimal, and there is deliberately no completion date — "you'd hit 30
    /// around 8 January" is not something a reader can act on, where "11 days
    /// a book to hit 30" is.
    private func footer(_ p: YearProjection) -> some View {
        HStack(alignment: .bottom, spacing: 10) {
            ForEach(p.stats) { stat in
                VStack(alignment: .leading, spacing: 4) {
                    Text(stat.value)
                        .font(.display(23))
                        .foregroundStyle(stat.isAccent ? palette.accentColor : palette.ink0Color)
                    Text(stat.label)
                        .font(.ui(11))
                        .foregroundStyle(palette.ink2Color)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .combine)
            }
        }
        .padding(.top, 14)
        .overlay(alignment: .top) { Hairline() }
        .padding(.top, 16)
    }
}

/// One figure under the curve.
struct YearStat: Identifiable, Hashable {
    let id: String
    let value: String
    let label: String
    var isAccent = false
}

/// Everything the year card draws, derived in one place from the summary.
///
/// All of it falls out of the monthly finished counts the payload already
/// carries: cumulative → pace → projection → the rate still needed. Editing
/// the target moves every figure, and nothing here is stored.
struct YearProjection {
    /// Books finished in each month of `year`, January first, twelve slots.
    /// Months after the current one are zero and are not drawn.
    let monthly: [Int64]
    /// Running total through the current month — the curve's own points.
    let cumulative: [Int64]
    /// Index of the current month, 0-based.
    let nowIndex: Int
    let dayOfYear: Int
    let daysInYear: Int
    let target: Int64?

    init(summary: StatsSummary, year: String) {
        let calendar = StatsFormat.utc
        let asOf = StatsFormat.wireDay.date(from: summary.asOfDay)

        var slots = [Int64](repeating: 0, count: 12)
        // Filtered to the goal's own year and re-indexed by month, rather than
        // taken as sent: `booksPerMonth` is the trailing twelve months, so it
        // opens in *last* year for any month before December.
        for point in summary.booksPerMonth where point.month.hasPrefix("\(year)-") {
            guard let month = Int(point.month.suffix(2)), (1...12).contains(month) else { continue }
            slots[month - 1] = point.books
        }
        monthly = slots

        let month = asOf.map { calendar.component(.month, from: $0) } ?? 12
        nowIndex = max(0, min(11, month - 1))
        dayOfYear = asOf.flatMap { calendar.ordinality(of: .day, in: .year, for: $0) } ?? 365
        daysInYear =
            asOf.flatMap { calendar.range(of: .day, in: .year, for: $0)?.count } ?? 365

        var running: Int64 = 0
        cumulative = slots.prefix(nowIndex + 1).map { books in
            running += books
            return running
        }
        target = summary.goal?.target
    }

    /// Books finished so far this year. Read off the same monthly series the
    /// curve is drawn from, so the headline figure and the dot on the curve
    /// cannot disagree — the server derives both from one completion
    /// definition, and taking one from `goal.current` would still let a
    /// rounding or ordering difference show up as a mismatch on screen.
    var finished: Int64 { cumulative.last ?? 0 }

    var daysLeft: Int { max(0, daysInYear - dayOfYear) }

    /// Days per book at the pace set so far, `nil` before the first book.
    var daysPerBook: Double? {
        guard finished > 0 else { return nil }
        return Double(dayOfYear) / Double(finished)
    }

    /// Books by 31 December if the year's pace holds.
    var projected: Int64 {
        guard let daysPerBook, daysPerBook > 0 else { return finished }
        return Int64((Double(finished) + Double(daysLeft) / daysPerBook).rounded())
    }

    var remaining: Int64 { target.map { max(0, $0 - finished) } ?? 0 }

    /// Days per book the rest of the year has to run at to reach the target.
    var neededDaysPerBook: Double? {
        guard remaining > 0 else { return nil }
        return Double(daysLeft) / Double(remaining)
    }

    var stats: [YearStat] {
        var out = [
            YearStat(
                id: "pace",
                value: daysPerBook.map { String(format: "%.1f", $0) } ?? "\u{2014}",
                label: "days a book, your pace"
            ),
            YearStat(id: "projected", value: "\(projected)", label: "books by 31 Dec"),
        ]
        guard let target else { return out }
        if let needed = neededDaysPerBook {
            out.append(
                YearStat(
                    id: "needed",
                    value: "\(Int(needed))",
                    label: "days a book to hit \(target)",
                    isAccent: true
                ))
        } else {
            out.append(
                YearStat(id: "met", value: "\u{2713}", label: "goal already met", isAccent: true))
        }
        return out
    }
}

/// The cumulative curve: solid through the current month, dashed to year end,
/// with the goal line and both endpoints labelled.
private struct YearGoalChart: View {
    let projection: YearProjection

    @Environment(\.palette) private var palette

    private static let box = CGSize(width: 330, height: 128)
    private static let monthInitials = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"]
    private static let yTop: CGFloat = 10
    private static let yBase: CGFloat = 110

    /// The vertical scale: whichever of the goal and the projection is higher,
    /// plus headroom so the top point never sits on the frame.
    private var yMax: CGFloat {
        CGFloat(max(projection.target ?? 0, projection.projected, 1)) + 2
    }

    private func x(_ index: Int) -> CGFloat { 8 + CGFloat(index) * 28.5 }

    private func y(_ value: CGFloat) -> CGFloat {
        Self.yBase - (value / yMax) * (Self.yBase - Self.yTop)
    }

    /// The measured months.
    private var actual: [CGPoint] {
        projection.cumulative.enumerated().map { CGPoint(x: x($0.offset), y: y(CGFloat($0.element))) }
    }

    /// Straight-line projection from here to 31 December.
    private var projected: [CGPoint] {
        let last = projection.cumulative.count - 1
        guard last < 11 else { return [] }
        let from = CGFloat(projection.finished)
        let to = CGFloat(projection.projected)
        return (last...11).map { index in
            let t = CGFloat(index - last) / CGFloat(11 - last)
            return CGPoint(x: x(index), y: y(from + (to - from) * t))
        }
    }

    var body: some View {
        GeometryReader { geometry in
            let scale = geometry.size.width / Self.box.width
            ZStack(alignment: .topLeading) {
                shapes(scale)
                labels(scale)
            }
            .frame(width: geometry.size.width, height: Self.box.height * scale)
        }
        .aspectRatio(Self.box.width / Self.box.height, contentMode: .fit)
        .accessibilityElement()
        .accessibilityLabel("Books finished this year, cumulative")
        .accessibilityValue(
            "\(projection.finished) so far, \(projection.projected) projected by year end")
    }

    private func shapes(_ scale: CGFloat) -> some View {
        let points = actual.map { CGPoint(x: $0.x * scale, y: $0.y * scale) }
        let forecast = projected.map { CGPoint(x: $0.x * scale, y: $0.y * scale) }
        let base = Self.yBase * scale

        return ZStack(alignment: .topLeading) {
            if let target = projection.target {
                Path { path in
                    let line = y(CGFloat(target)) * scale
                    path.move(to: CGPoint(x: 0, y: line))
                    path.addLine(to: CGPoint(x: Self.box.width * scale, y: line))
                }
                .stroke(
                    palette.ink3Color,
                    style: StrokeStyle(lineWidth: 1, dash: [3, 4])
                )
            }

            // The area first, so both strokes sit on top of their own fill.
            Path { path in
                guard let first = points.first, let last = points.last else { return }
                path.move(to: first)
                points.dropFirst().forEach { path.addLine(to: $0) }
                path.addLine(to: CGPoint(x: last.x, y: base))
                path.addLine(to: CGPoint(x: first.x, y: base))
                path.closeSubpath()
            }
            .fill(palette.accentColor.opacity(0.16))

            Path { path in
                guard let first = forecast.first else { return }
                path.move(to: first)
                forecast.dropFirst().forEach { path.addLine(to: $0) }
            }
            .stroke(
                palette.accentColor.opacity(0.55),
                style: StrokeStyle(lineWidth: 2, lineCap: .round, dash: [4, 4])
            )

            Path { path in
                guard let first = points.first else { return }
                path.move(to: first)
                points.dropFirst().forEach { path.addLine(to: $0) }
            }
            .stroke(
                palette.accentColor,
                style: StrokeStyle(lineWidth: 2.4, lineCap: .round, lineJoin: .round)
            )

            if let now = points.last {
                Circle()
                    .fill(palette.accentColor)
                    .frame(width: 7.2, height: 7.2)
                    .position(now)
            }
            if let end = forecast.last {
                Circle()
                    .fill(palette.bg0Color)
                    .overlay(Circle().strokeBorder(palette.accentColor, lineWidth: 2))
                    .frame(width: 8, height: 8)
                    .position(end)
            }
        }
    }

    private func labels(_ scale: CGFloat) -> some View {
        ZStack(alignment: .topLeading) {
            if let target = projection.target {
                // Anchored at the *left* end of the goal line rather than the
                // right: the projection dot and its own value sit at the right
                // edge, and whenever the projection lands near the target —
                // which is exactly when a reader looks — the two labels
                // printed over each other.
                Text("goal \(target)")
                    .font(.monoUI(8.5))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize()
                    .frame(width: Self.box.width * scale, alignment: .leading)
                    .position(
                        x: Self.box.width * scale / 2,
                        y: (y(CGFloat(target)) - 9) * scale
                    )
            }

            if let end = projected.last {
                Text("\(projection.projected)")
                    .font(.monoUI(9, weight: .bold))
                    .foregroundStyle(palette.ink1Color)
                    .fixedSize()
                    .position(x: (end.x - 10) * scale, y: (end.y - 12) * scale)
            }

            ForEach(Array(Self.monthInitials.enumerated()), id: \.offset) { index, initial in
                Text(initial)
                    .font(.monoUI(8.5))
                    .foregroundStyle(
                        index == projection.nowIndex ? palette.accentColor : palette.ink3Color
                    )
                    .fixedSize()
                    .position(x: x(index) * scale, y: 122 * scale)
            }
        }
        .allowsHitTesting(false)
    }
}
