//  StatsWindow.swift
//  Everything the period control governs, and the control itself.
//
//  The load-bearing decision of the redesign is that the switcher governs only
//  part of the page and the page says so. That is why the control lives here,
//  at the boundary of the band it moves, rather than in the masthead: a
//  control in the header claims the whole screen, and half of this screen is
//  standing figures a period switch must never touch.

import SwiftUI

// MARK: - The boundary

/// The band's own header: the accent label, the window it currently names, and
/// the four-up control.
///
/// Pinned by the enclosing `Section`, so it holds at the top of the scroll
/// while you are inside the band and releases exactly as the standing rule
/// reaches it. A control that stayed pinned over the standing sections would
/// be lying about what it affects.
struct WindowBandHeader: View {
    @Binding var range: StatsRange
    let caption: String

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                StatsSectionLabel("In this window", color: palette.accentColor)
                Spacer(minLength: Spacing.sm)
                Text(caption)
                    .font(.ui(12))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }

            RangeControl(range: $range)
        }
        .screenPadding()
        .padding(.top, 10)
        .padding(.bottom, 12)
        // Sized to the header and no further. An earlier version ran the
        // ground upward to cover the safe-area band above it — but a header is
        // laid out in the scroll's own flow, so that background painted over
        // the section *before* it and swallowed the finished-books rail. What
        // shows in that band is the scrim's job, not this one's.
        .background(palette.bg0Color)
        // Only once it has actually pinned would a shadow be right, and there
        // is no scroll-state hook for that on a pinned header; a hairline
        // reads as a boundary either way and never as a floating card.
        .overlay(alignment: .bottom) { Hairline() }
    }
}

/// The rule that closes the windowed band.
///
/// A header rather than ordinary content, so that reaching it is what
/// displaces `WindowBandHeader` — and once pinned it keeps saying, over every
/// section beneath it, that the control above no longer applies.
struct StandingBandHeader: View {
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            StatsSectionLabel("Not tied to the window")
            Hairline()
        }
        .screenPadding()
        .padding(.top, 10)
        .padding(.bottom, 12)
        .background(palette.bg0Color)
    }
}

/// Week / Month / Year / All, as a segmented control rather than a menu.
///
/// A menu hides the options behind a tap and puts them somewhere other than
/// the data; four 44pt targets sitting directly above the figures they move
/// keep the headline and the tiles on screen while you change them.
struct RangeControl: View {
    @Binding var range: StatsRange

    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: 4) {
            ForEach(StatsRange.allCases, id: \.self) { option in
                Button {
                    withAnimation(Motion.snap) { range = option }
                } label: {
                    Text(Self.shortLabel(option))
                        .font(.ui(12.5, weight: range == option ? .semibold : .medium))
                        .foregroundStyle(range == option ? palette.ink0Color : palette.ink2Color)
                        .frame(maxWidth: .infinity)
                        .frame(minHeight: 44)
                        .background(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .fill(range == option ? palette.bg3Color : .clear)
                                .shadow(
                                    color: .black.opacity(range == option ? 0.4 : 0),
                                    radius: 3, y: 1
                                )
                        )
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(range == option ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(3)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.bg1Color)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(palette.line2.color, lineWidth: 0.5)
        )
        .accessibilityLabel("Time range")
    }

    /// "Lifetime" doesn't fit a quarter of a phone's width; the other three
    /// are already their own short forms.
    static func shortLabel(_ range: StatsRange) -> String {
        range == .allTime ? "All" : range.label
    }
}

// MARK: - Headline

/// The one number anybody opens this tab for, with the split drawn along the
/// card's foot so the read-listen ratio is visible without reading the caption.
struct WindowHeadline: View {
    let summary: StatsSummary
    let eyebrow: String

    @Environment(\.palette) private var palette

    var body: some View {
        StatsCard {
            VStack(alignment: .leading, spacing: 0) {
                Text(eyebrow)
                    .font(.monoUI(10, weight: .bold))
                    .tracking(0.8)
                    .textCase(.uppercase)
                    .foregroundStyle(palette.accentColor)

                Text(Format.humanDuration(summary.totalSeconds))
                    .font(.display(52))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                    .minimumScaleFactor(0.5)
                    .contentTransition(.numericText())
                    .padding(.top, 8)

                Text(Self.splitLabel(summary))
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 8)
            }
        }
        .overlay(alignment: .bottom) {
            // Only when there are two things to split — a full-width bar
            // stating "100% reading" is a decoration that looks like
            // information.
            if summary.readingSeconds > 0, summary.listeningSeconds > 0 {
                SplitBar(reading: summary.readingSeconds, listening: summary.listeningSeconds)
                    .clipShape(
                        UnevenRoundedRectangle(
                            bottomLeadingRadius: Radius.lg,
                            bottomTrailingRadius: Radius.lg,
                            style: .continuous
                        )
                    )
            }
        }
        .animation(Motion.settle, value: summary.totalSeconds)
        .screenPadding()
    }

    /// The headline already states the total, so the caption breaks it down
    /// only when there is a breakdown — otherwise it restated the same number
    /// one line below itself ("11m" over "11m reading").
    static func splitLabel(_ summary: StatsSummary) -> String {
        guard summary.totalSeconds > 0 else { return "Nothing logged in this range yet" }

        var parts: [String] = []
        if summary.readingSeconds > 0, summary.listeningSeconds > 0 {
            parts.append("\(Format.humanDuration(summary.readingSeconds)) reading")
            parts.append("\(Format.humanDuration(summary.listeningSeconds)) listening")
        } else {
            parts.append(summary.listeningSeconds > 0 ? "Listening" : "Reading")
        }
        if summary.sessions > 0 {
            parts.append(StatsFormat.counted(summary.sessions, "session"))
        }
        if summary.activeDays > 0 {
            parts.append(StatsFormat.counted(summary.activeDays, "day"))
        }
        return parts.joined(separator: " \u{b7} ")
    }
}

/// Reading against listening as one two-tone rule.
struct SplitBar: View {
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

// MARK: - Tiles

/// One windowed figure: its symbol and name above, its value and the change
/// against the previous window below.
struct WindowTile: View {
    let label: String
    let value: String
    let icon: String
    var delta: String?

    @Environment(\.palette) private var palette

    private var isEmpty: Bool { value == "\u{2014}" }

    var body: some View {
        StatsCard(padding: 0) {
            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 7) {
                    Image(systemName: icon)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(palette.ink3Color)
                    Text(label)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink2Color)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)
                }

                HStack(alignment: .firstTextBaseline, spacing: 7) {
                    // A serif em dash at 30pt draws a 30pt rule, which reads
                    // as a divider rather than as "no value". The mono face
                    // keeps the placeholder the size of a character.
                    Text(value)
                        .font(isEmpty ? .monoUI(22) : .display(30, weight: .semibold))
                        .foregroundStyle(isEmpty ? palette.ink3Color : palette.ink0Color)
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                        .contentTransition(.numericText())
                    Spacer(minLength: 0)
                    if let delta {
                        Text(delta)
                            .font(.monoUI(10))
                            .foregroundStyle(palette.accentColor)
                            .layoutPriority(1)
                    }
                }
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 14)
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: - When you read

/// The day as a 24-hour dial, with the weekday strip beneath it.
///
/// Chosen over the two `Charts` strips it replaces because a day is a cycle:
/// on a linear axis "late evening" is a bar at the right-hand edge and its
/// neighbour at 1am is at the far left, which is exactly the wrong shape for
/// the question. Zero hours draw a stub in the inert tone rather than nothing,
/// so the ring stays a ring.
struct ReadingClock: View {
    let summary: StatsSummary

    @Environment(\.palette) private var palette

    private static let diameter: CGFloat = 148
    /// Where a tick's inner end sits. The longest tick reaches 43 + 28 = 71,
    /// just inside the 74pt radius.
    private static let innerRadius: CGFloat = 43
    private static let maxTick: CGFloat = 28

    private var peak: Int64 { max(1, summary.hourOfDay.map(\.seconds).max() ?? 1) }
    private var hasHours: Bool { summary.hasTimePatterns }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if hasHours {
                HStack(alignment: .center, spacing: 16) {
                    dial
                    Text(Self.clockLine(summary))
                        .font(.display(18))
                        .foregroundStyle(palette.ink0Color)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                // Both the dial and the strip are fixed-width, so drawing them
                // here would show a measured-looking day made entirely of
                // zeros.
                Text("No activity with a recorded local time in this period yet.")
                    .font(.ui(13))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize(horizontal: false, vertical: true)
            }

            // Gated on `hasHours`, not just on the strip being non-empty: the
            // server zero-fills all seven buckets, so a window whose activity
            // is all unzoned would otherwise print "no activity with a
            // recorded local time" and then draw seven flat bars under it —
            // the measured-looking day made of zeros `TimePatternCharts`
            // guarded against before this replaced it.
            if hasHours, !summary.dayOfWeek.isEmpty {
                weekdays
                    .padding(.top, 16)
                    .overlay(alignment: .top) { Hairline() }
                    .padding(.top, 18)
            }

            // Stated rather than absorbed: bucketing sessions that carry no
            // capture-time zone as UTC would put a reader's evening at 4am.
            if summary.unzonedSeconds > 0 {
                Text(
                    "\(Format.humanDuration(summary.unzonedSeconds)) of activity was recorded "
                        + "without a timezone and isn\u{2019}t shown here."
                )
                .font(.ui(12))
                .foregroundStyle(palette.ink3Color)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 14)
                .overlay(alignment: .top) { Hairline() }
                .padding(.top, 16)
            }
        }
    }

    private var dial: some View {
        ZStack {
            ForEach(summary.hourOfDay) { bucket in
                let fraction = Double(bucket.seconds) / Double(peak)
                let length = max(4, fraction * Self.maxTick)
                Capsule()
                    .fill(Self.tickColor(fraction, palette: palette))
                    .frame(width: 6, height: length)
                    // Hour 0 at twelve o'clock, running clockwise: the inner
                    // end is pinned at `innerRadius` so every tick grows
                    // outward from one concentric ring rather than from its
                    // own centre.
                    .offset(y: -(Self.innerRadius + length / 2))
                    .rotationEffect(.degrees(Double(bucket.hour) * 15))
                    .accessibilityLabel(bucket.clockLabel)
                    .accessibilityValue(Format.humanDuration(bucket.seconds))
            }

            Circle()
                .strokeBorder(palette.line2.color, lineWidth: 0.5)
                .frame(width: 72, height: 72)
                .overlay {
                    VStack(spacing: 4) {
                        Text(Self.peakLabel(summary))
                            .font(.display(23))
                            .foregroundStyle(palette.accentColor)
                        Text("peak")
                            .font(.monoUI(8))
                            .tracking(0.8)
                            .textCase(.uppercase)
                            .foregroundStyle(palette.ink3Color)
                    }
                }
        }
        .frame(width: Self.diameter, height: Self.diameter)
        .animation(Motion.settle, value: summary.range)
    }

    private var weekdays: some View {
        VStack(alignment: .leading, spacing: 9) {
            Text("Day of week")
                .font(.monoUI(10))
                .foregroundStyle(palette.ink3Color)

            let dayPeak = max(1, summary.dayOfWeek.map(\.seconds).max() ?? 1)
            ForEach(summary.dayOfWeek) { bucket in
                HStack(spacing: 10) {
                    Text(bucket.label)
                        .font(.monoUI(10))
                        .foregroundStyle(palette.ink3Color)
                        .frame(width: 26, alignment: .leading)

                    StatsBar(
                        fraction: Double(bucket.seconds) / Double(dayPeak),
                        color: bucket.seconds == dayPeak
                            ? palette.accentColor : StatsRamp.c1.color,
                        track: palette.bg2Color
                    )

                    // A rest day is "—", never "0h 0m": zero minutes read as a
                    // measurement of a day nothing happened on.
                    Text(bucket.seconds > 0 ? Format.humanDuration(bucket.seconds) : "\u{2014}")
                        .font(.monoUI(10))
                        .foregroundStyle(bucket.seconds > 0 ? palette.ink2Color : palette.ink3Color)
                        .frame(width: 44, alignment: .trailing)
                }
                .accessibilityElement(children: .combine)
            }
        }
    }

    /// The tick's tone by how much of the peak hour it holds. Four steps, not
    /// a continuous ramp: a gradient across 24 ticks reads as one smear, where
    /// steps let a reader see the shape of an evening.
    static func tickColor(_ fraction: Double, palette: Palette) -> Color {
        switch fraction {
        case let f where f > 0.7: palette.accentColor
        case let f where f > 0.35: StatsRamp.c1.color
        case let f where f > 0.05: StatsRamp.c2.color
        default: palette.bg3Color
        }
    }

    /// The busiest hour, as a clock reads it. `—` when the window has no
    /// placeable activity: an all-zero array's first maximum is index 0, so
    /// the naive answer to an empty window is "midnight".
    static func peakLabel(_ summary: StatsSummary) -> String {
        guard summary.hasTimePatterns,
            let peak = summary.hourOfDay.max(by: { $0.seconds < $1.seconds })
        else { return "\u{2014}" }
        let hour = Int(peak.hour)
        return "\(hour % 12 == 0 ? 12 : hour % 12)\(hour < 12 ? "am" : "pm")"
    }

    /// One derived sentence about the shape of the day: which quarter carries
    /// the reading, and by how much.
    ///
    /// Derived rather than written, because it has to stay true on every
    /// window and for every reader — a fixed line like "you read late" is
    /// wrong for half of them and wrong for the other half in March.
    static func clockLine(_ summary: StatsSummary) -> String {
        let bands: [(name: String, clause: String, hours: Range<Int64>)] = [
            ("Nights", "between midnight and six", 0..<6),
            ("Mornings", "before noon", 6..<12),
            ("Afternoons", "between noon and six", 12..<18),
            ("Evenings", "after six", 18..<24),
        ]
        let total = summary.hourOfDay.reduce(0) { $0 + $1.seconds }
        guard total > 0 else { return "Nothing placeable on a clock yet." }

        let scored = bands.map { band in
            (band, summary.hourOfDay.filter { band.hours.contains($0.hour) }
                .reduce(0) { $0 + $1.seconds })
        }
        guard let (band, seconds) = scored.max(by: { $0.1 < $1.1 }), seconds > 0 else {
            return "Nothing placeable on a clock yet."
        }
        let share = Int((Double(seconds) / Double(total) * 100).rounded())
        return "\(band.name) carry it \u{2014} \(share)% of your time falls \(band.clause)."
    }
}

// MARK: - How you spent it

/// The genre mix as a donut, centred on the books it can actually speak for.
struct GenreDonut: View {
    let summary: StatsSummary

    @Environment(\.palette) private var palette

    private var slices: [GenreSlice] { Self.slices(summary.genreShare, palette: palette) }

    var body: some View {
        StatsCard {
            HStack(alignment: .center, spacing: 18) {
                donut
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(slices) { slice in
                        HStack(spacing: 9) {
                            RoundedRectangle(cornerRadius: 2, style: .continuous)
                                .fill(slice.color)
                                .frame(width: 9, height: 9)
                            Text(slice.name)
                                .font(.ui(12.5))
                                .foregroundStyle(palette.ink1Color)
                                .lineLimit(1)
                            Spacer(minLength: Spacing.sm)
                            Text("\(slice.percent)%")
                                .font(.monoUI(10))
                                .foregroundStyle(palette.ink2Color)
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        }
    }

    private var donut: some View {
        ZStack {
            Circle()
                .fill(
                    AngularGradient(
                        stops: Self.stops(slices),
                        center: .center,
                        // A conic gradient starts at three o'clock; a donut
                        // reads from twelve.
                        angle: .degrees(-90)
                    )
                )
            Circle()
                .fill(palette.bg1Color)
                .frame(width: 104 * 0.62, height: 104 * 0.62)
            VStack(spacing: 2) {
                // `verbatim`, like the goal ring's figure: a plain
                // interpolation takes the `LocalizedStringKey` path and groups
                // the integer, so a well-tagged library read "1,204" inside a
                // 64pt hole beside ungrouped copy.
                Text(verbatim: "\(summary.genreTaggedBooks)")
                    .font(.display(22, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                Text("tagged")
                    .font(.monoUI(8))
                    .foregroundStyle(palette.ink3Color)
            }
        }
        .frame(width: 104, height: 104)
        .accessibilityElement()
        .accessibilityLabel("Genre mix")
        .accessibilityValue(
            "\(summary.genreTaggedBooks) books with a genre. "
                + slices.map { "\($0.name) \($0.percent) percent" }.joined(separator: ", "))
    }

    /// The top four genres plus a remainder, as whole percents of the genre
    /// assignments in the window.
    ///
    /// The share is over assignments, not over books: a book carrying three
    /// genres is in three slices, which is why the centre reports
    /// `genreTaggedBooks` — the distinct books the ring describes — rather
    /// than a total the slices would overstate.
    static func slices(_ share: [GenreShare], palette: Palette) -> [GenreSlice] {
        let total = share.reduce(0) { $0 + $1.books }
        guard total > 0 else { return [] }
        let colors = [palette.accentColor, StatsRamp.c1.color, StatsRamp.c2.color,
                      StatsRamp.c3.color]

        var out = share.prefix(4).enumerated().map { index, entry in
            GenreSlice(
                name: entry.name,
                percent: Int((Double(entry.books) / Double(total) * 100).rounded()),
                color: colors[index]
            )
        }
        let rest = share.dropFirst(4).reduce(0) { $0 + $1.books }
        if rest > 0 {
            out.append(
                GenreSlice(
                    name: "Other",
                    percent: Int((Double(rest) / Double(total) * 100).rounded()),
                    color: StatsRamp.quiet.color
                ))
        }
        return out
    }

    /// Cumulative hard stops, so a slice rounded to zero collapses to no width
    /// at all rather than leaving a hairline of colour behind.
    static func stops(_ slices: [GenreSlice]) -> [Gradient.Stop] {
        var out: [Gradient.Stop] = []
        var running = 0.0
        let total = max(1.0, Double(slices.reduce(0) { $0 + $1.percent }))
        for slice in slices {
            let from = running / total
            running += Double(slice.percent)
            let to = running / total
            out.append(.init(color: slice.color, location: from))
            out.append(.init(color: slice.color, location: to))
        }
        return out
    }
}

/// One wedge of the genre donut and its row in the key.
struct GenreSlice: Identifiable, Hashable {
    let name: String
    let percent: Int
    let color: Color

    var id: String { name }
}

// MARK: - The standouts

/// The window's most-X rows, in the same order as the web card.
struct StandoutsCard: View {
    let rows: [StandoutRow]
    let showFastestReadNote: Bool

    @Environment(\.palette) private var palette

    var body: some View {
        StatsCard(padding: 0) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(row.label)
                                .font(.ui(11.5))
                                .foregroundStyle(palette.ink3Color)
                            Text(row.headline)
                                .font(.display(19))
                                .foregroundStyle(palette.ink0Color)
                                .lineLimit(2)
                        }
                        Spacer(minLength: Spacing.sm)
                        Text(row.detail)
                            .font(.monoUI(10.5))
                            .foregroundStyle(palette.ink2Color)
                            .layoutPriority(1)
                    }
                    .padding(.vertical, 13)
                    .overlay(alignment: .top) { if index > 0 { Hairline() } }
                    .accessibilityElement(children: .combine)
                }

                if showFastestReadNote {
                    // The floor is part of the claim, not an aside: without it
                    // a book read mostly on another device reads as a sprint.
                    Text(
                        "Fastest read counts days from your first tracked session, over books "
                            + "with at least "
                            + "\(Format.humanDuration(Superlatives.fastestReadMinSeconds)) of "
                            + "recorded time — reading done before tracking, or on a device that "
                            + "reports nothing, can only make a book look faster than it was."
                    )
                    .font(.ui(11))
                    .foregroundStyle(palette.ink3Color)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 14)
                    .padding(.bottom, 4)
                    .overlay(alignment: .top) { Hairline() }
                }
            }
            .padding(.horizontal, Spacing.lg)
            .padding(.vertical, 4)
        }
    }
}
