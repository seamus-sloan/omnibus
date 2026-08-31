//  BookDetailStops.swift
//  The seven snap-stop panels of the book detail marquee, the small pieces
//  they share (kicker, ruler, stat tiles, cover strips, inset lists), and the
//  pure derivations behind them (`DetailRead`, `DetailStats`).

import SwiftUI

// MARK: - Pure derivations

/// Copy and numbers the stops derive from the record — pure, so the suite can
/// assert them without a screen.
enum DetailRead {
    /// The Home stop's kicker line: where this book sits in the catalog.
    static func kicker(
        series: String?, seriesIndex: String?, fallback: String?, year: String?
    ) -> String {
        var lead: String
        if let series {
            lead = series
            if let seriesIndex { lead += " · Book \(seriesIndex)" }
        } else if let fallback {
            lead = "\(fallback) · standalone"
        } else {
            lead = "In your library"
        }
        if let year { lead += " · \(year)" }
        return lead
    }

    /// The action bar's primary label. Speaks the position when one is saved
    /// — the reading one when reading has started, else the listening one, so
    /// a dual-format book someone has only listened to doesn't read as
    /// unstarted. Falls back to naming the act.
    static func resumeLabel(
        hasEbook: Bool, hasAudiobook: Bool, epubStarted: Bool,
        epubPercent: Int64?, audioSeconds: Double?
    ) -> String {
        if hasEbook, let epubPercent, epubPercent > 0 {
            return "Resume — \(epubPercent)%"
        }
        // Reading has started but the save carries no percent (a bare CFI):
        // still a resume, just one with no number to speak.
        if hasEbook, epubStarted {
            return "Resume"
        }
        if hasAudiobook, let audioSeconds, audioSeconds > 1 {
            return "Resume — \(Format.humanDuration(Int64(audioSeconds)))"
        }
        if hasEbook { return "Read" }
        if hasAudiobook { return "Listen" }
        return "Open"
    }

    /// Whether the primary CTA opens the player rather than the reader —
    /// kept in lockstep with `resumeLabel`, so a label speaking an audio
    /// position never opens the reader at page one.
    static func resumesIntoPlayer(
        hasEbook: Bool, hasAudiobook: Bool, epubStarted: Bool, audioSeconds: Double?
    ) -> Bool {
        guard hasAudiobook else { return false }
        guard hasEbook else { return true }
        return !epubStarted && (audioSeconds ?? 0) > 1
    }

    /// Whole-book fraction for the Home ruler. The reading percent wins;
    /// with no reading record at all the listening position stands in.
    /// `nil` when no honest fraction exists — a bare CFI carries no percent,
    /// and audio needs a measured duration.
    static func fraction(
        epubStarted: Bool, epubPercent: Int64?, audioSeconds: Double?, audioDuration: Double?
    ) -> Double? {
        if let epubPercent {
            return min(1, max(0, Double(epubPercent) / 100))
        }
        // A CFI-only save means reading is underway somewhere the percent
        // can't say — drawing the (older) audio position would misplace it.
        guard !epubStarted else { return nil }
        guard let audioSeconds, let audioDuration, audioDuration > 0 else { return nil }
        return min(1, max(0, audioSeconds / audioDuration))
    }
}

/// What the stats stop states about this read, folded from the session log.
struct DetailReadRecord: Equatable {
    var startedAt: Int64
    var daysIn: Int
    var totalSeconds: Int64
    var sessions: Int
    var averageSeconds: Int64
    var longestSeconds: Int64
    var longestAt: Int64
    var readSeconds: Int64
    var listenSeconds: Int64
}

enum DetailStats {
    /// Folds a per-book session log (any order) into the stop's record.
    /// `nil` when there are no sittings — the stop states that instead.
    static func record(
        from sessions: [SessionLogEntry], now: Date = Date()
    ) -> DetailReadRecord? {
        guard !sessions.isEmpty else { return nil }

        let started = sessions.map(\.startedAt).min() ?? 0
        let total = sessions.reduce(0) { $0 + $1.seconds }
        let longest = sessions.max { $0.seconds < $1.seconds }
        let read = sessions.filter { $0.format != .listening }.reduce(0) { $0 + $1.seconds }
        let listen = sessions.filter { $0.format != .reading }.reduce(0) { $0 + $1.seconds }
        let days = max(
            1, Int((now.timeIntervalSince1970 - Double(started)) / 86_400.0)
        )
        return DetailReadRecord(
            startedAt: started,
            daysIn: days,
            totalSeconds: total,
            sessions: sessions.count,
            averageSeconds: total / Int64(sessions.count),
            longestSeconds: longest?.seconds ?? 0,
            longestAt: longest?.startedAt ?? 0,
            readSeconds: read,
            listenSeconds: listen
        )
    }

    /// Minutes of activity per calendar day for the trailing `days` days,
    /// oldest first — the spark strip. Always exactly `days` entries.
    /// Bucketed on `startOfDay`, not trailing 24-hour windows: "today" means
    /// the local calendar day, and DST days keep their sittings.
    static func sparkMinutes(
        from sessions: [SessionLogEntry], days: Int = 21, now: Date = Date(),
        calendar: Calendar = .current
    ) -> [Int] {
        var buckets = [Int](repeating: 0, count: days)
        let today = calendar.startOfDay(for: now)
        for session in sessions {
            let day = calendar.startOfDay(
                for: Date(timeIntervalSince1970: Double(session.startedAt)))
            guard let offset = calendar.dateComponents([.day], from: day, to: today).day,
                  offset >= 0, offset < days
            else { continue }
            buckets[days - 1 - offset] += Int(session.seconds / 60)
        }
        return buckets
    }
}

// MARK: - Shared pieces

/// The "01 / 07 — Home" line every stop leads with.
struct StopLabel: View {
    let stop: DetailStop

    @Environment(\.palette) private var palette

    var body: some View {
        (Text(String(format: "%02d / %02d — ", stop.rawValue + 1, DetailStop.allCases.count))
            .foregroundStyle(palette.ink3Color)
            + Text(stop.name.uppercased())
            .foregroundStyle(palette.ink1Color))
            .font(.monoUI(9.5))
            .tracking(1.6)
            .padding(.bottom, 14)
            .accessibilityAddTraits(.isHeader)
    }
}

/// The accent mono kicker a stop's content opens with.
struct DetailKicker: View {
    let text: String

    @Environment(\.palette) private var palette

    var body: some View {
        Text(text.uppercased())
            .font(.monoUI(9.5))
            .tracking(1.6)
            .foregroundStyle(palette.accentColor)
            .lineLimit(1)
    }
}

/// Small mono footnote line.
struct MonoNote: View {
    let text: String
    var color: Color?

    @Environment(\.palette) private var palette

    var body: some View {
        Text(text)
            .font(.monoUI(9.5))
            .foregroundStyle(color ?? palette.ink3Color)
            .lineSpacing(3)
    }
}

/// The shape of what a stop will hold, drawn as blank rules.
///
/// Every stop gets a whole screen, so one with nothing in it was one line of
/// italic text above eight hundred points of black. Rather than pad that out,
/// the stop shows the *form* its rows take — the same tick and hairline the
/// real ones draw — fading down the page. It reads as a ruled page waiting to
/// be written on rather than as a load that failed, and it costs nothing when
/// the stop does have content, because it isn't drawn then.
struct StopRuledVoid: View {
    var rows = 3
    /// Highlights carry a colour tick down their leading edge; journal rows
    /// don't. Matching it is what keeps this reading as *this* stop's shape.
    var ticked = false

    @Environment(\.palette) private var palette

    private static let barHeight: CGFloat = 5
    private static let barGap: CGFloat = 8

    var body: some View {
        VStack(spacing: 0) {
            ForEach(0 ..< rows, id: \.self) { row in
                VStack(spacing: 0) {
                    HStack(alignment: .top, spacing: 11) {
                        if ticked {
                            Capsule()
                                .fill(palette.line.color)
                                .frame(width: 3, height: 30)
                        }
                        bars
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 12)

                    Hairline()
                }
                // Fades out down the page, so the rules read as the page
                // continuing rather than as N specific missing rows. Spread
                // across `rows - 1` so the *last* row lands at 0.25 whatever
                // the arity — dividing by `rows` would make a two-row void
                // end brighter than a three-row one. The fixed 0.3 step this
                // replaced went negative past four rows.
                .opacity(1 - (Double(row) / Double(max(rows - 1, 1))) * 0.75)
            }
        }
        .accessibilityHidden(true)
    }

    /// The two blank rules of one row.
    ///
    /// One `GeometryReader` for the pair rather than one each: it is the only
    /// way to take a *fraction* of the row's width, but it has no intrinsic
    /// height, so every use has to be given one back — and stating that once
    /// per row beats stating it once per bar.
    private var bars: some View {
        GeometryReader { geometry in
            VStack(alignment: .leading, spacing: Self.barGap) {
                bar(geometry.size.width * 0.82)
                bar(geometry.size.width * 0.54)
            }
        }
        .frame(height: Self.barHeight * 2 + Self.barGap)
    }

    private func bar(_ width: CGFloat) -> some View {
        Capsule()
            .fill(palette.line2.color)
            .frame(width: width, height: Self.barHeight)
    }
}

/// A single-line horizontal chip shelf that scrolls past the panel edge
/// instead of wrapping — vertical space at a stop is fixed.
struct ChipStrip<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 7) { content() }
        }
        .scrollIndicators(.hidden)
        .scrollClipDisabled()
    }
}

/// Catalog genre chip — accent-tinted, the book's own vocabulary.
struct GenreChip: View {
    let label: String

    @Environment(\.palette) private var palette

    var body: some View {
        Text(label)
            .font(.ui(12, weight: .medium))
            .foregroundStyle(palette.ink0Color)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(Capsule().fill(palette.accentColor.opacity(0.10)))
            .overlay(
                Capsule().strokeBorder(palette.accentColor.opacity(0.55), lineWidth: 0.5)
            )
    }
}

/// Reader tag chip — dashed, mono, the reader's own vocabulary.
struct TagChip: View {
    let label: String

    @Environment(\.palette) private var palette

    var body: some View {
        Text("#\(label)")
            .font(.monoUI(10))
            .foregroundStyle(palette.ink2Color)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .overlay(
                Capsule().strokeBorder(
                    palette.line.color,
                    style: StrokeStyle(lineWidth: 1, dash: [4, 3])
                )
            )
    }
}

/// The reading-status control, styled as the system segmented control: a
/// translucent track with a raised selected segment.
struct DetailSegmented: View {
    @Binding var selection: ReadStatus
    var onChange: (ReadStatus) -> Void

    @Environment(\.palette) private var palette
    @Namespace private var indicator

    var body: some View {
        HStack(spacing: 2) {
            ForEach(ReadStatus.allCases, id: \.self) { status in
                segment(status)
            }
        }
        .padding(2)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(palette.ink0Color.opacity(0.08))
        )
    }

    private func segment(_ status: ReadStatus) -> some View {
        let isOn = selection == status

        return Button {
            guard selection != status else { return }
            Haptics.select()
            withAnimation(Motion.snap) { selection = status }
            onChange(status)
        } label: {
            Text(status.label)
                .font(.ui(13, weight: isOn ? .semibold : .regular))
                .foregroundStyle(isOn ? palette.ink0Color : palette.ink2Color)
                .lineLimit(1)
                .frame(maxWidth: .infinity)
                .frame(height: 32)
                .background {
                    if isOn {
                        RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .fill(palette.bg2Color.opacity(0.94))
                            .shadow(color: .black.opacity(0.35), radius: 3, y: 1)
                            .matchedGeometryEffect(id: "seg", in: indicator)
                    }
                }
                .contentShape(RoundedRectangle(cornerRadius: 7))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? [.isSelected, .isButton] : .isButton)
    }
}

/// The position ruler: a track, the accent fill, and a flagged caret.
struct DetailRuler: View {
    let fraction: Double
    var flag: String?
    var left: String?
    var right: String?

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            GeometryReader { geometry in
                let width = geometry.size.width
                let x = width * min(1, max(0, fraction))

                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(palette.bg2Color)

                    RoundedRectangle(cornerRadius: 3)
                        .fill(
                            LinearGradient(
                                colors: [palette.accentColor.opacity(0.7), palette.accentColor],
                                startPoint: .leading,
                                endPoint: .trailing
                            )
                        )
                        .frame(width: max(3, x))

                    if fraction < 1 {
                        Rectangle()
                            .fill(palette.ink0Color)
                            .frame(width: 1.5, height: 18)
                            .offset(x: max(0, min(width - 2, x)), y: -3)

                        if let flag {
                            Text(flag)
                                .font(.monoUI(8.5))
                                .foregroundStyle(palette.ink1Color)
                                .fixedSize()
                                .position(x: max(20, min(width - 20, x)), y: -13)
                        }
                    }
                }
            }
            .frame(height: 12)
            .padding(.top, flag == nil ? 0 : 18)

            if left != nil || right != nil {
                HStack {
                    if let left { MonoNote(text: left) }
                    Spacer(minLength: Spacing.sm)
                    if let right { MonoNote(text: right) }
                }
            }
        }
        .accessibilityElement()
        .accessibilityLabel("Position")
        .accessibilityValue("\(Int(min(1, max(0, fraction)) * 100)) percent")
    }
}

/// One big stat: mono key, display-face value, mono footnote.
struct DetailStatTile: View {
    let key: String
    let value: String
    var sub: String?

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(key.uppercased())
                .font(.monoUI(9))
                .tracking(1.1)
                .foregroundStyle(palette.ink3Color)
            Text(value)
                .font(.display(29))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            if let sub {
                Text(sub)
                    .font(.monoUI(9.5))
                    .foregroundStyle(palette.ink2Color)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Daily-minutes bars for the trailing three weeks.
struct SparkBars: View {
    let minutes: [Int]

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 6) {
            HStack(alignment: .bottom, spacing: 3) {
                let peak = max(minutes.max() ?? 1, 1)
                ForEach(Array(minutes.enumerated()), id: \.offset) { _, value in
                    RoundedRectangle(cornerRadius: 1)
                        .fill(value > 0 ? palette.accentColor : palette.bg2Color)
                        .frame(height: max(2, CGFloat(value) / CGFloat(peak) * 34))
                        .frame(maxWidth: .infinity)
                }
            }
            .frame(height: 34, alignment: .bottom)

            HStack {
                MonoNote(text: "3 wk ago")
                Spacer()
                MonoNote(text: "minutes · by day")
                Spacer()
                MonoNote(text: "today")
            }
        }
        .accessibilityHidden(true)
    }
}

/// One cover in a horizontal shelf strip.
struct StripCover: View {
    let book: Book
    var width: CGFloat = 96
    var sub: String?
    var dimmed = false

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            BookCover(identity: CoverIdentity(book), size: .md, cornerRadius: 6)
                .frame(width: width)
                .coverShadow(0.7)
            Text(book.displayTitle)
                .font(.ui(11))
                .foregroundStyle(palette.ink1Color)
                .lineLimit(1)
            if let sub {
                Text(sub)
                    .font(.monoUI(9))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }
        }
        .frame(width: width, alignment: .leading)
        .opacity(dimmed ? 0.82 : 1)
    }
}

/// The inset grouped list — native iOS's vocabulary for "rows of a record".
struct InsetList<Content: View>: View {
    @ViewBuilder var content: () -> Content

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) { content() }
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(palette.bg1Color.opacity(0.6))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 0.5)
            )
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

struct InsetRow<Icon: View, Trailing: View>: View {
    let name: String
    var sub: String?
    var isFirst = false
    var chevron = true
    var action: (() -> Void)?
    @ViewBuilder var icon: () -> Icon
    @ViewBuilder var trailing: () -> Trailing

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            if !isFirst {
                Hairline().padding(.leading, 55)
            }

            Button {
                guard let action else { return }
                Haptics.tap()
                action()
            } label: {
                HStack(spacing: 12) {
                    icon()
                        .frame(width: 30, height: 30)
                        .background(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(palette.bg2Color)
                        )
                        .foregroundStyle(palette.ink1Color)

                    VStack(alignment: .leading, spacing: 3) {
                        Text(name)
                            .font(.ui(14))
                            .foregroundStyle(palette.ink0Color)
                        if let sub {
                            Text(sub)
                                .font(.monoUI(9.5))
                                .foregroundStyle(palette.ink3Color)
                                .lineLimit(1)
                        }
                    }

                    Spacer(minLength: Spacing.sm)

                    trailing()

                    if chevron {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(palette.ink3Color.opacity(0.7))
                    }
                }
                .padding(.horizontal, 13)
                .padding(.vertical, 10)
                .frame(minHeight: 48)
                .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle())
            .disabled(action == nil)
        }
    }
}

/// The accent "All N …" affordance that carries a stop's overflow to a sheet.
struct MoreRowButton: View {
    let label: String
    var identifier: String?
    let action: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            VStack(spacing: 0) {
                Hairline()
                HStack(spacing: 10) {
                    Text(label)
                        .font(.ui(13.5))
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .semibold))
                        .opacity(0.7)
                }
                .foregroundStyle(palette.accentColor)
                .padding(.top, 12)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.top, 12)
        .accessibilityIdentifier(identifier ?? "")
    }
}

// MARK: - 01 · Home

struct StopHome: View {
    let book: Book
    let model: BookDetailModel
    var onMore: () -> Void
    var onRemovedWishlist: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            DetailKicker(text: DetailRead.kicker(
                series: book.series,
                seriesIndex: book.seriesIndex,
                fallback: book.genres.first ?? book.subjects.first,
                year: book.year
            ))

            Text(book.displayTitle)
                .font(.display(38, weight: .semibold))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(3)
                .minimumScaleFactor(0.6)
                .padding(.top, 10)

            Text("by \(book.authorDisplay)")
                .font(.ui(13.5))
                .foregroundStyle(palette.ink1Color)
                .lineLimit(1)
                .padding(.top, 7)

            if !book.genres.isEmpty || !book.subjects.isEmpty {
                ChipStrip {
                    ForEach(book.genres, id: \.self) { GenreChip(label: $0) }
                    ForEach(book.subjects, id: \.self) { subject in
                        NavigationLink(value: Destination.tag(name: subject)) {
                            TagChip(label: subject)
                        }
                        .buttonStyle(PressableStyle())
                    }
                }
                .padding(.top, 11)
                .accessibilityIdentifier("book-detail-genres")
            }

            if let description = book.description?.nilIfBlank {
                Text(description.strippingHTML)
                    .font(.ui(13))
                    .foregroundStyle(palette.ink1Color)
                    .lineSpacing(4)
                    .lineLimit(2)
                    .padding(.top, 11)
                    .onTapGesture { onMore() }

                Button {
                    Haptics.tap()
                    onMore()
                } label: {
                    Text("MORE")
                        .font(.monoUI(9.5))
                        .tracking(1.5)
                        .foregroundStyle(palette.accentColor)
                }
                .buttonStyle(.plain)
                .padding(.top, 6)
            }

            if model.isWishlistOnly {
                if let entry = model.wishlistEntry {
                    WishlistSection(book: book, entry: entry, onRemoved: onRemovedWishlist)
                        .padding(.top, 16)
                }
            } else {
                DetailSegmented(
                    selection: Binding(
                        get: { model.readStatus },
                        set: { model.readStatus = $0 }
                    )
                ) { status in
                    Task { await model.setStatus(status, uuid: book.uuid) }
                }
                .padding(.top, 14)

                ruler
                    .padding(.top, 17)
            }
        }
    }

    @ViewBuilder
    private var ruler: some View {
        let fraction = DetailRead.fraction(
            epubStarted: model.epubProgress != nil,
            epubPercent: model.epubProgress?.progressPercent,
            audioSeconds: model.audioProgress?.audioPositionSeconds,
            audioDuration: model.audioDuration
        )

        if let fraction {
            let percent = Int(fraction * 100)
            DetailRuler(
                fraction: fraction,
                flag: "\(percent)%",
                left: leftLabel(fraction: fraction),
                right: updatedLabel
            )
        } else if model.epubProgress != nil || model.audioProgress != nil {
            // A position exists but supports no honest bar (a bare CFI, or
            // audio with no measured duration) — say so instead of "unread".
            MonoNote(text: ["in progress", updatedLabel].compactMap { $0 }
                .joined(separator: " · "))
        } else {
            MonoNote(text: book.hasEbook || book.hasAudiobook ? "not started yet" : " ")
        }
    }

    /// Matches `DetailRead.fraction`'s source: percent text when the bar
    /// draws the reading percent, listening time when it draws the audio
    /// position.
    private func leftLabel(fraction: Double) -> String {
        if model.epubProgress?.progressPercent == nil,
           let seconds = model.audioProgress?.audioPositionSeconds,
           let total = model.audioDuration
        {
            return "\(Format.humanDuration(Int64(seconds))) of \(Format.humanDuration(Int64(total)))"
        }
        return "\(Int(fraction * 100))% read"
    }

    private var updatedLabel: String? {
        let record = model.epubProgress ?? model.audioProgress
        guard let clock = record?.orderingClock else { return nil }
        return "updated \(Format.relative(unix: clock))"
    }
}

// MARK: - 02 · Shelf

struct StopShelf: View {
    let book: Book
    let model: BookDetailModel
    var onShelfPicker: () -> Void

    @Environment(\.palette) private var palette

    /// The series in reading order, when the book belongs to one the library
    /// holds more of.
    private var series: [Book] {
        model.seriesBooks.sorted {
            (Double($0.seriesIndex ?? "") ?? 0) < (Double($1.seriesIndex ?? "") ?? 0)
        }
    }

    private var shelfNames: [String] {
        model.allShelves
            .filter { model.shelvesContaining.contains($0.id) }
            .map(\.name)
    }

    /// Whether "this book is on none of your shelves" is a thing we can yet
    /// say. `allShelves` and `shelvesContaining` are separate live reads, so
    /// on a cold cache the stop renders before either lands — and asserting
    /// "· none" then told the reader something the app did not know, and took
    /// it back a moment later when the chips appeared. Derived from the data
    /// rather than a load flag: naming the shelves a book is on requires the
    /// shelf catalog, so having it *is* the precondition.
    private var knowsMembership: Bool { !model.allShelves.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if series.count > 1, let name = book.series {
                DetailKicker(text: "\(name) · you own \(series.count)")

                ScrollView(.horizontal) {
                    HStack(alignment: .top, spacing: 12) {
                        ForEach(series) { member in
                            if member.id == book.id {
                                StripCover(
                                    book: member, width: 118,
                                    sub: member.seriesIndex.map { "Book \($0) · this book" }
                                )
                            } else {
                                NavigationLink(value: Destination.book(uuid: member.uuid)) {
                                    StripCover(
                                        book: member, width: 118,
                                        sub: member.seriesIndex.map { "Book \($0)" },
                                        dimmed: true
                                    )
                                }
                                .buttonStyle(PressableStyle())
                            }
                        }
                    }
                }
                .scrollIndicators(.hidden)
                .scrollClipDisabled()
                .padding(.top, 12)

                if let seriesId = book.seriesId {
                    NavigationLink(value: Destination.series(id: seriesId)) {
                        MonoNote(text: "series page →", color: palette.accentColor)
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 14)
                }
            } else {
                DetailKicker(text: knowsMembership && shelfNames.isEmpty
                    ? "On your shelves · none"
                    : shelfNames.isEmpty
                        ? "On your shelves"
                        : "On your shelves · \(shelfNames.count)")
            }

            // A lone "+ Shelf" chip under a bare kicker said nothing about
            // what a shelf is or why this book has none.
            if series.count <= 1, knowsMembership, shelfNames.isEmpty {
                Text("This book isn't on a shelf yet.")
                    .font(.displayItalic(21))
                    .foregroundStyle(palette.ink2Color)
                    .lineSpacing(4)
                    .padding(.top, 12)
            }

            ChipStrip {
                ForEach(shelfNames, id: \.self) { name in
                    Chip(label: name)
                }
                Button {
                    Haptics.tap()
                    onShelfPicker()
                } label: {
                    Chip(label: "+ Shelf")
                        .opacity(0.75)
                }
                .buttonStyle(PressableStyle())
            }
            .padding(.top, series.count > 1 ? 18 : 12)

            if series.count <= 1, knowsMembership, model.authorBooks.isEmpty, shelfNames.isEmpty {
                MonoNote(text: "a shelf gathers books by hand, or fills itself from a rule")
                    .padding(.top, 20)
                StopRuledVoid(rows: 2)
                    .padding(.top, 18)
            }

            if series.count <= 1, !model.authorBooks.isEmpty {
                MonoNote(text: "more by \(book.authorDisplay)")
                    .padding(.top, 20)
                ScrollView(.horizontal) {
                    HStack(alignment: .top, spacing: 12) {
                        ForEach(model.authorBooks.prefix(8)) { other in
                            NavigationLink(value: Destination.book(uuid: other.uuid)) {
                                StripCover(book: other, width: 96)
                            }
                            .buttonStyle(PressableStyle())
                        }
                    }
                }
                .scrollIndicators(.hidden)
                .scrollClipDisabled()
                .padding(.top, 10)
            }
        }
    }
}

// MARK: - 03 · Stats

struct StopStats: View {
    let book: Book
    let model: BookDetailModel

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let record = DetailStats.record(from: model.sessions) {
                DetailKicker(text: "What this read has looked like")

                statGrid(record)
                    .padding(.top, 16)

                SparkBars(minutes: DetailStats.sparkMinutes(from: model.sessions))
                    .padding(.top, 18)
            } else {
                DetailKicker(text: "This read · not begun")

                Text("No stats yet.")
                    .font(.display(34))
                    .foregroundStyle(palette.ink0Color)
                    .padding(.top, 10)

                Text(emptyExplainer)
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink2Color)
                    .lineSpacing(4)
                    .padding(.top, 12)

                // The four tiles this stop fills in, drawn empty — so the
                // shape of what a read records is legible before there is one.
                emptyStatGrid
                    .padding(.top, 20)
            }

            ratingBlock
                .padding(.top, 20)

            if !model.otherRatings.isEmpty {
                otherReaders
                    .padding(.top, 16)
            }
        }
    }

    private var emptyExplainer: String {
        model.isWishlistOnly
            ? "Stats begin when the book does — check in a copy to start the record."
            : "Open the book to start tracking your reading here."
    }

    /// The stat grid with its keys shown and its values withheld — the same
    /// four tiles, so the layout doesn't jump the first time a sitting lands.
    private var emptyStatGrid: some View {
        VStack(spacing: 16) {
            HStack(spacing: 14) {
                DetailStatTile(key: "Started", value: "—", sub: nil)
                DetailStatTile(key: "Time in book", value: "—", sub: nil)
            }
            HStack(spacing: 14) {
                DetailStatTile(key: "Pickups", value: "—", sub: nil)
                DetailStatTile(key: "Longest sit", value: "—", sub: nil)
            }
        }
        .opacity(0.45)
    }

    private func statGrid(_ record: DetailReadRecord) -> some View {
        let both = record.readSeconds > 0 && record.listenSeconds > 0
        let timeKey = both || record.readSeconds > 0 ? "Time in book" : "Time listened"

        return VStack(spacing: 16) {
            HStack(spacing: 14) {
                DetailStatTile(
                    key: "Started",
                    value: Format.date(unix: record.startedAt),
                    sub: "\(record.daysIn) days in"
                )
                DetailStatTile(
                    key: timeKey,
                    value: Format.humanDuration(record.totalSeconds),
                    sub: both ? "ebook + audio" : nil
                )
            }
            HStack(spacing: 14) {
                DetailStatTile(
                    key: "Pickups",
                    value: "\(record.sessions)",
                    sub: "avg sit \(Format.humanDuration(record.averageSeconds))"
                )
                DetailStatTile(
                    key: "Longest sit",
                    value: Format.humanDuration(record.longestSeconds),
                    sub: Format.date(unix: record.longestAt)
                )
            }
        }
    }

    private var ratingBlock: some View {
        HStack(spacing: 12) {
            StarRating(stars: model.rating, size: 19, interactive: true) { stars in
                Task {
                    if stars > 0 {
                        await model.setRating(stars, uuid: book.uuid)
                    } else {
                        await model.clearRating(uuid: book.uuid)
                    }
                }
            }
            MonoNote(
                text: model.rating > 0
                    ? "rated \(model.rating.formatted()) of 5"
                    : "not rated yet"
            )
        }
        .animation(Motion.snap, value: model.rating)
    }

    private var otherReaders: some View {
        VStack(alignment: .leading, spacing: 9) {
            ForEach(model.otherRatings.prefix(3)) { rating in
                HStack(spacing: 9) {
                    UserAvatar(
                        id: rating.userId,
                        name: rating.username,
                        hasAvatar: rating.hasAvatar,
                        size: 20
                    )
                    Text(rating.username)
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink1Color)
                    Spacer(minLength: Spacing.sm)
                    StarRating(stars: rating.stars, size: 11)
                }
            }
        }
    }
}

// MARK: - 04 · Highlights

struct StopHighlights: View {
    let book: Book
    let model: BookDetailModel
    var onAll: () -> Void

    /// Passages shown on the stop before the sheet takes over.
    static let stopCount = 4

    /// Newest-first, capped for the stop.
    static func preview(of highlights: [Highlight]) -> [Highlight] {
        Array(highlights.sorted { $0.createdAt > $1.createdAt }.prefix(stopCount))
    }

    @Environment(\.palette) private var palette

    var body: some View {
        let all = model.highlights

        VStack(alignment: .leading, spacing: 0) {
            if all.isEmpty {
                DetailKicker(text: "Kept lines · none")
                Text("Highlight while you read to keep lines here.")
                    .font(.displayItalic(21))
                    .foregroundStyle(palette.ink2Color)
                    .lineSpacing(4)
                    .padding(.top, 12)
                if model.isWishlistOnly {
                    MonoNote(text: "find a copy first — check in when it arrives")
                        .padding(.top, 16)
                }
                StopRuledVoid(ticked: true)
                    .padding(.top, 22)
            } else {
                DetailKicker(text: "Kept lines · \(all.count)")

                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Self.preview(of: all)) { highlight in
                        HighlightRow(highlight: highlight)
                    }
                }
                .padding(.top, 6)

                MoreRowButton(
                    label: "All \(all.count) highlights",
                    identifier: "highlights-show-more"
                ) { onAll() }
            }
        }
    }
}

/// One kept line: the passage in the italic display cut, its color as a
/// slim tick, and when it was kept.
struct HighlightRow: View {
    let highlight: Highlight
    var large = false

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                if let text = highlight.text?.nilIfBlank {
                    Text(text)
                        .font(.displayItalic(large ? 17 : 15.5))
                        .foregroundStyle(palette.ink1Color)
                        .lineSpacing(4)
                        .lineLimit(large ? nil : 3)
                }
                if let note = highlight.note?.nilIfBlank {
                    Text(note)
                        .font(.ui(12))
                        .foregroundStyle(palette.ink2Color)
                        .lineLimit(large ? nil : 2)
                }
                MonoNote(text: "kept \(Format.relative(unix: highlight.createdAt))")
            }
            .padding(.leading, 11)
            .overlay(alignment: .leading) {
                Capsule()
                    .fill(highlight.color.tint)
                    .frame(width: 3)
            }
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)

            Hairline()
        }
    }
}

// MARK: - 05 · Journals

struct StopJournals: View {
    let book: Book
    let model: BookDetailModel
    var onWrite: () -> Void
    var onOpen: (JournalEntry) -> Void
    var onAll: () -> Void

    /// Entries listed on the stop before the sheet takes over.
    static let stopCount = 4

    @Environment(\.palette) private var palette

    var body: some View {
        let entries = model.journals

        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                DetailKicker(text: entries.isEmpty
                    ? "The journal · empty"
                    : "The journal · \(entries.count) \(entries.count == 1 ? "entry" : "entries")")
                Spacer(minLength: Spacing.sm)
                writePill
            }

            if entries.isEmpty {
                Text("No entries yet — the journal begins when you do.")
                    .font(.displayItalic(21))
                    .foregroundStyle(palette.ink2Color)
                    .lineSpacing(4)
                    .padding(.top, 12)
                MonoNote(text: "a shared log — anyone reading this book can write here")
                    .padding(.top, 16)
                StopRuledVoid()
                    .padding(.top, 22)
            } else {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(entries.prefix(Self.stopCount)) { entry in
                        JournalRow(entry: entry) { onOpen(entry) }
                    }
                }
                .padding(.top, 8)

                if entries.count > Self.stopCount {
                    MoreRowButton(label: "All \(entries.count) entries") { onAll() }
                } else {
                    MonoNote(text: "everyone reading this book writes here")
                        .padding(.top, 11)
                }
            }
        }
    }

    private var writePill: some View {
        Button {
            Haptics.tap()
            onWrite()
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "square.and.pencil")
                    .font(.system(size: 10, weight: .semibold))
                Text("WRITE")
                    .font(.monoUI(9.5))
                    .tracking(1.3)
            }
            .foregroundStyle(palette.accentColor)
            .padding(.horizontal, 13)
            .frame(height: 30)
            .background(Capsule().fill(palette.accentColor.opacity(0.14)))
            .overlay(Capsule().strokeBorder(palette.accentColor.opacity(0.45), lineWidth: 0.5))
        }
        .buttonStyle(PressableStyle())
    }
}

/// One journal row: who, where they were, and the opening line.
struct JournalRow: View {
    let entry: JournalEntry
    var onOpen: () -> Void

    @Environment(\.palette) private var palette

    /// The opening line as plain text. Parsed as markdown rather than
    /// regex-stripped, so prose that happens to contain `#` or `_` (C#,
    /// snake_case) keeps its characters while real syntax is dropped.
    static func preview(_ md: String) -> String {
        guard let first = md.split(separator: "\n").first else { return "" }
        let line = String(first)
            // A leading list or heading marker is block syntax the inline
            // parser would pass through verbatim.
            .replacingOccurrences(
                of: #"^\s*([-*+]\s+|#{1,6}\s+)"#, with: "", options: .regularExpression)
        let parsed = (try? AttributedString(
            markdown: line,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )).map { String($0.characters) }
        return (parsed ?? line).trimmingCharacters(in: .whitespaces)
    }

    var body: some View {
        Button {
            Haptics.tap()
            onOpen()
        } label: {
            VStack(spacing: 0) {
                HStack(alignment: .top, spacing: 11) {
                    UserAvatar(
                        id: entry.authorId,
                        name: entry.authorName,
                        hasAvatar: entry.authorHasAvatar,
                        size: 26
                    )
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            Text(entry.authorName)
                                .font(.ui(12, weight: .medium))
                                .foregroundStyle(palette.ink0Color)
                            if let progress = entry.progress {
                                Text("— at \(progress)%")
                                    .font(.monoUI(9))
                                    .foregroundStyle(palette.ink3Color)
                            }
                        }
                        Text(Self.preview(entry.bodyMd))
                            .font(.ui(12.5))
                            .foregroundStyle(palette.ink1Color)
                            .lineSpacing(3)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(palette.ink3Color.opacity(0.7))
                        .padding(.top, 5)
                }
                .padding(.vertical, 11)

                Hairline()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableStyle())
    }
}

// MARK: - 06 · The files

struct StopFiles: View {
    let book: Book
    let model: BookDetailModel
    var onRead: () -> Void
    var onListen: () -> Void
    var onAlignment: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            DetailKicker(text: "Every way you hold this book")

            InsetList {
                if book.hasEbook {
                    InsetRow(
                        name: "Ebook", sub: ebookSub, isFirst: true, action: onRead
                    ) {
                        Image(systemName: "book")
                            .font(.system(size: 14))
                    } trailing: {
                        DownloadBadge(book: book, kind: .ebook)
                    }
                }
                if book.hasAudiobook {
                    InsetRow(
                        name: "Audiobook", sub: audioSub,
                        isFirst: !book.hasEbook, action: onListen
                    ) {
                        Image(systemName: "headphones")
                            .font(.system(size: 14))
                    } trailing: {
                        DownloadBadge(book: book, kind: .audio)
                    }
                }
                if book.hasPhysical {
                    InsetRow(
                        name: "Physical copy",
                        sub: book.isbn13.map { "ISBN \($0)" } ?? "On your shelf",
                        isFirst: !book.hasEbook && !book.hasAudiobook,
                        chevron: false
                    ) {
                        Image(systemName: "books.vertical")
                            .font(.system(size: 13))
                    } trailing: {
                        Circle()
                            .fill(palette.okColor)
                            .frame(width: 7, height: 7)
                    }
                }
            }
            .padding(.top, 4)

            if book.hasEbook, book.hasAudiobook {
                Button {
                    Haptics.tap()
                    onAlignment()
                } label: {
                    MonoNote(text: "⇄ \(syncLabel)", color: palette.accentColor)
                }
                .buttonStyle(.plain)
                .padding(.top, 10)
                .padding(.leading, 4)
                .accessibilityIdentifier("position-sync-row")
            }

            metaGrid
                .padding(.top, 16)
        }
    }

    private var ebookSub: String {
        let formats = book.formats
            .filter { Book.ebookFormats.contains($0.lowercased()) }
            .map { $0.uppercased() }
            .joined(separator: " · ")
        if let size = book.epubSizeBytes {
            return "\(formats) · \(Format.bytes(size))"
        }
        return formats
    }

    private var audioSub: String {
        let files = book.audioFiles
        let format = files.first?.format.uppercased() ?? "AUDIO"
        let bytes = files.reduce(0) { $0 + $1.sizeBytes }
        var sub = files.count > 1 ? "\(format) · \(files.count) files" : format
        if let duration = model.audioDuration {
            sub += " · \(Format.humanDuration(Int64(duration)))"
        }
        if bytes > 0 {
            sub += " · \(Format.bytes(bytes))"
        }
        return sub
    }

    private var syncLabel: String {
        switch model.syncState {
        case .linkStale: "position sync — needs re-confirm"
        case .notLinked, nil: "position sync — off"
        default: "positions linked"
        }
    }

    /// The colophon, as the design's kv grid: mono keys, right-aligned values.
    private var metaGrid: some View {
        VStack(spacing: 0) {
            kvRow("Publisher", book.publisher)
            kvRow("Published", book.published.map(Format.looseDate))
            kvRow("Language", book.language)
            kvRow("ISBN", book.isbn13)
            kvRow("Added", book.addedAt.map(Format.isoDate))
            kvRow("File", book.filename)
        }
    }

    @ViewBuilder
    private func kvRow(_ key: String, _ value: String?) -> some View {
        if let value = value?.nilIfBlank {
            VStack(spacing: 0) {
                Hairline()
                HStack(alignment: .firstTextBaseline, spacing: Spacing.md) {
                    Text(key.uppercased())
                        .font(.monoUI(9.5))
                        .tracking(0.5)
                        .foregroundStyle(palette.ink3Color)
                    Spacer(minLength: Spacing.md)
                    Text(value)
                        .font(.monoUI(10.5))
                        .foregroundStyle(palette.ink1Color)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
                .padding(.vertical, 8)
            }
        }
    }
}

/// The download state for one format, compact enough to sit in a list row:
/// a control to fetch, progress while moving, a check once held. Remove and
/// re-download live in the row's context menu.
struct DownloadBadge: View {
    let book: Book
    let kind: DownloadKind

    @Environment(\.palette) private var palette
    private var downloads = DownloadManager.shared

    init(book: Book, kind: DownloadKind) {
        self.book = book
        self.kind = kind
    }

    var body: some View {
        let record = downloads.record(for: book.uuid, kind: kind)

        Group {
            switch record?.state {
            case .complete where downloads.isStale(book.uuid, kind: kind, against: book):
                Button {
                    Haptics.tap()
                    Task { await downloads.redownload(book: book, kind: kind) }
                } label: {
                    Image(systemName: "arrow.triangle.2.circlepath")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.warnColor)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Update download")

            case .complete:
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 15))
                    .foregroundStyle(palette.okColor)
                    .contextMenu {
                        Button(role: .destructive) {
                            Task { await downloads.remove(book.uuid, kind: kind) }
                        } label: {
                            Label("Remove download", systemImage: "trash")
                        }
                    }
                    .accessibilityLabel("Downloaded")

            case .running, .queued:
                Button {
                    Task { await downloads.cancel(book.uuid, kind: kind) }
                } label: {
                    ProgressView(value: record?.fraction ?? 0)
                        .progressViewStyle(.circular)
                        .controlSize(.small)
                        .tint(palette.accentColor)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Downloading — tap to cancel")

            case .failed:
                // Visibly a failure, not a fresh download — and the reason
                // travels with it, so the tap that retries isn't a mystery.
                Button {
                    Haptics.tap()
                    Task { await downloads.start(book: book, kind: kind) }
                } label: {
                    HStack(spacing: 5) {
                        if let message = record?.error {
                            Text(message)
                                .font(.monoUI(9))
                                .foregroundStyle(palette.badColor)
                                .lineLimit(1)
                                .frame(maxWidth: 130, alignment: .trailing)
                        }
                        Image(systemName: "exclamationmark.arrow.triangle.2.circlepath")
                            .font(.system(size: 15, weight: .medium))
                            .foregroundStyle(palette.badColor)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Download failed — retry")

            default:
                Button {
                    Haptics.tap()
                    Task { await downloads.start(book: book, kind: kind) }
                } label: {
                    Image(systemName: "arrow.down.circle")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundStyle(palette.accentColor)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Download")
            }
        }
    }
}

// MARK: - 07 · Recommendations

struct StopRecommendations: View {
    let book: Book
    let model: BookDetailModel

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let authorId = book.creators.first?.id {
                InsetList {
                    NavigationLink(value: Destination.author(id: authorId)) {
                        HStack(spacing: 12) {
                            authorDisc
                            VStack(alignment: .leading, spacing: 3) {
                                Text(book.authorDisplay)
                                    .font(.ui(14))
                                    .foregroundStyle(palette.ink0Color)
                                    .lineLimit(1)
                                Text("you own \(model.authorBooks.count + 1) · author page")
                                    .font(.monoUI(9.5))
                                    .foregroundStyle(palette.ink3Color)
                            }
                            Spacer(minLength: Spacing.sm)
                            Image(systemName: "chevron.right")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(palette.ink3Color.opacity(0.7))
                        }
                        .padding(.horizontal, 13)
                        .padding(.vertical, 10)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(PressableStyle())
                }
            }

            if !model.authorBooks.isEmpty {
                ScrollView(.horizontal) {
                    HStack(alignment: .top, spacing: 12) {
                        ForEach(model.authorBooks.prefix(8)) { other in
                            NavigationLink(value: Destination.book(uuid: other.uuid)) {
                                StripCover(book: other, width: 80)
                            }
                            .buttonStyle(PressableStyle())
                        }
                    }
                }
                .scrollIndicators(.hidden)
                .scrollClipDisabled()
                .padding(.top, 16)
            }

            HStack(alignment: .firstTextBaseline) {
                DetailKicker(text: "Suggested for you")
                Spacer(minLength: Spacing.sm)
                MonoNote(text: "via Hardcover")
            }
            .padding(.top, 18)

            if model.suggestions.isEmpty {
                MonoNote(text: "nothing suggested for this book yet")
                    .padding(.top, 10)
                StopRuledVoid(rows: 2)
                    .padding(.top, 14)
            } else {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(model.suggestions.prefix(4)) { suggestion in
                        VStack(spacing: 0) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(suggestion.title)
                                    .font(.ui(14))
                                    .foregroundStyle(palette.ink0Color)
                                    .lineLimit(1)
                                if let author = suggestion.author {
                                    Text(author)
                                        .font(.monoUI(9.5))
                                        .foregroundStyle(palette.ink3Color)
                                        .lineLimit(1)
                                }
                                if let reason = suggestion.reason?.nilIfBlank {
                                    Text(reason)
                                        .font(.ui(11.5))
                                        .foregroundStyle(palette.ink2Color)
                                        .lineLimit(2)
                                }
                            }
                            .padding(.vertical, 9)
                            .frame(maxWidth: .infinity, alignment: .leading)

                            Hairline()
                        }
                    }
                }
                .padding(.top, 8)
            }
        }
    }

    private var authorDisc: some View {
        Circle()
            .fill(palette.accentColor.opacity(0.22))
            .frame(width: 34, height: 34)
            .overlay(
                Circle().strokeBorder(palette.accentColor.opacity(0.4), lineWidth: 0.5)
            )
            .overlay {
                Text(String(book.authorDisplay.prefix(1)))
                    .font(.displayItalic(16))
                    .foregroundStyle(palette.accentColor)
            }
    }
}

// MARK: - Sheets

/// The full jacket blurb — the Home stop clamps it to two lines.
struct DescriptionDrawer: View {
    let book: Book

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text(book.displayTitle)
                    .font(.ui(14, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                MonoNote(text: [book.authorDisplay, book.year].compactMap { $0 }
                    .joined(separator: " · "))
            }
            .padding(.horizontal, 18)
            .padding(.top, 22)
            .padding(.bottom, 12)

            ScrollView {
                Text((book.description ?? "").strippingHTML)
                    .font(.display(17))
                    .foregroundStyle(palette.ink1Color)
                    .lineSpacing(6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 18)
                    .padding(.bottom, 34)
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(palette.bg1Color)
    }
}

/// Every kept line, newest first — where the Highlights stop overflows to.
struct AllHighlightsSheet: View {
    let book: Book
    let highlights: [Highlight]

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Highlights · \(book.displayTitle)")
                    .font(.ui(14, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                MonoNote(text: "\(highlights.count) kept lines · newest first")
            }
            .padding(.horizontal, 18)
            .padding(.top, 22)
            .padding(.bottom, 8)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(highlights.sorted { $0.createdAt > $1.createdAt }) { highlight in
                        HighlightRow(highlight: highlight, large: true)
                    }
                }
                .padding(.horizontal, 18)
                .padding(.bottom, 34)
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .presentationBackground(palette.bg1Color)
    }
}

/// Every journal entry, in full — where the Journals stop overflows to.
/// Tapping an entry hands it back to the drawer, where Edit and Delete live.
struct AllJournalsSheet: View {
    let book: Book
    let entries: [JournalEntry]
    var onOpen: (JournalEntry) -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text("The journal · \(book.displayTitle)")
                    .font(.ui(14, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                MonoNote(text: "\(entries.count) entries · everyone reading this book")
            }
            .padding(.horizontal, 18)
            .padding(.top, 22)
            .padding(.bottom, 8)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 26) {
                    ForEach(entries) { entry in
                        Button {
                            Haptics.tap()
                            onOpen(entry)
                        } label: {
                            VStack(alignment: .leading, spacing: 9) {
                                JournalByline(entry: entry, avatarSize: 26)
                                JournalBody(md: entry.bodyMd)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(PressableStyle())
                    }
                }
                .padding(.horizontal, 18)
                .padding(.top, 10)
                .padding(.bottom, 34)
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .presentationBackground(palette.bg1Color)
    }
}

/// One entry, in the drawer the Journals stop opens: takes the height it
/// needs, caps at the medium detent, leaves the stop visible behind it.
struct JournalDrawer: View {
    let entry: JournalEntry
    let isMine: Bool
    var onEdit: () -> Void
    var onDelete: () -> Void

    @Environment(\.palette) private var palette
    @State private var confirmDelete = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            JournalByline(entry: entry, avatarSize: 34)
                .padding(.horizontal, 18)
                .padding(.top, 20)
                .padding(.bottom, 12)

            ScrollView {
                JournalBody(md: entry.bodyMd)
                    .padding(.horizontal, 18)
                    .padding(.bottom, 20)
            }

            if isMine {
                HStack(spacing: 9) {
                    Button {
                        Haptics.tap()
                        onEdit()
                    } label: {
                        Text("Edit")
                    }
                    .buttonStyle(BarCTAStyle())

                    Button {
                        confirmDelete = true
                    } label: {
                        Text("Delete")
                            .font(.ui(14, weight: .medium))
                            .foregroundStyle(palette.badColor)
                            .frame(maxWidth: .infinity)
                            .frame(height: 50)
                            .background(
                                RoundedRectangle(cornerRadius: 14, style: .continuous)
                                    .fill(palette.bg2Color)
                            )
                    }
                    .buttonStyle(PressableStyle())
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .background {
                    palette.bg2Color.opacity(0.4)
                        .overlay(alignment: .top) { Hairline() }
                        .ignoresSafeArea(edges: .bottom)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(palette.bg1Color)
        .confirmationDialog(
            "Delete this entry?", isPresented: $confirmDelete, titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) { onDelete() }
            Button("Cancel", role: .cancel) {}
        }
    }
}

/// Who wrote an entry, and where they were in the book.
struct JournalByline: View {
    let entry: JournalEntry
    var avatarSize: CGFloat

    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: 10) {
            UserAvatar(
                id: entry.authorId,
                name: entry.authorName,
                hasAvatar: entry.authorHasAvatar,
                size: avatarSize
            )
            VStack(alignment: .leading, spacing: 3) {
                Text(entry.authorName)
                    .font(.ui(14, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                MonoNote(text: bylineDetail)
            }
        }
    }

    private var bylineDetail: String {
        var parts: [String] = []
        if let progress = entry.progress { parts.append("at \(progress)%") }
        parts.append(Format.date(unix: entry.createdAt))
        if entry.status == .draft { parts.append("draft") }
        return parts.joined(separator: " · ")
    }
}

/// A journal body in the reading face. Bodies are markdown; inline-only
/// rendering keeps the author's line breaks instead of collapsing the entry
/// into one paragraph.
struct JournalBody: View {
    let md: String

    @Environment(\.palette) private var palette

    private var rendered: AttributedString {
        (try? AttributedString(
            markdown: md,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )) ?? AttributedString(md)
    }

    var body: some View {
        Text(rendered)
            .font(.display(18))
            .foregroundStyle(palette.ink1Color)
            .lineSpacing(6)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - Small shared extensions

extension HighlightColor {
    var tint: Color {
        switch self {
        case .amber: OKLCH(0.80, 0.14, 80).color
        case .green: OKLCH(0.78, 0.13, 150).color
        case .blue: OKLCH(0.72, 0.12, 250).color
        case .rose: OKLCH(0.72, 0.15, 15).color
        case .violet: OKLCH(0.70, 0.14, 300).color
        }
    }
}

extension String {
    /// Book descriptions arrive as OPF HTML fragments.
    var strippingHTML: String {
        replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression)
            .replacingOccurrences(of: "&nbsp;", with: " ")
            .replacingOccurrences(of: "&amp;", with: "&")
            .replacingOccurrences(of: "&lt;", with: "<")
            .replacingOccurrences(of: "&gt;", with: ">")
            .replacingOccurrences(of: "&#39;", with: "'")
            .replacingOccurrences(of: "&quot;", with: "\"")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
