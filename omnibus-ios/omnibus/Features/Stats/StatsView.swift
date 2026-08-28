//  StatsView.swift
//  Reading stats: a headline panel, supporting tiles, an activity heatmap,
//  rankings, the finished-books rail, and the per-sitting session log behind
//  all of them.

import Charts
import SwiftUI

struct StatsView: View {
    @Environment(\.palette) private var palette

    @State private var range: StatsRange = .month
    @State private var summary: StatsSummary?
    /// Fetched separately from `summary`: library-scoped rather than
    /// per-user, so it must not re-fetch when the range picker moves.
    @State private var librarySize: LibrarySize?
    /// Fetched separately for the same reason: what the collection is *made
    /// of* is a library-wide answer that only moves on a reindex.
    @State private var libraryComposition: LibraryComposition?
    @State private var isLoading = true
    @State private var error: String?
    @State private var isEditingGoal = false
    @State private var goalDraft = ""
    @State private var goalError: String?
    // The log is its own paged read rather than a rollup of the window above
    // it, so it holds its own state and never reloads on a range change.
    @State private var sessions: [SessionLogEntry] = []
    @State private var sessionCursor: String?
    @State private var sessionsLoading = false
    @State private var sessionsError: String?

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
            .refreshTask {
                await load(force: true)
                await loadSessions()
            }
            .withDestinations()
        }
        .task {
            await load()
            await loadLibrarySize()
            await loadLibraryComposition()
            await loadSessions()
        }
        .onChange(of: range) { _, _ in Task { await load() } }
    }

    private func content(_ summary: StatsSummary) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 30) {
                Masthead(title: "Stats") { rangeMenu }

                // Above the range menu's own section stack and never keyed on
                // it: the goal is annual, so a period switch must not move it.
                if let year = goalYear(summary) {
                    goalCard(summary.goal, year: year)
                }

                headline(summary)
                tiles(summary)

                if let note = Self.pagesCutoverNote(summary) {
                    Text(note)
                        .font(.footnote)
                        .foregroundStyle(palette.ink3Color)
                        .screenPadding()
                }

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

                // Only when something was actually rated: ten flat bars
                // describe an empty window less honestly than no chart does.
                if summary.ratingHistogram.contains(where: { $0.books > 0 }) {
                    section("How you rated them") {
                        Chart(summary.ratingHistogram) { bucket in
                            BarMark(
                                x: .value("Rating", bucket.starLabel),
                                y: .value("Books", bucket.books)
                            )
                            .foregroundStyle(palette.accentColor)
                            .cornerRadius(3)
                        }
                        .chartXAxis {
                            AxisMarks { value in
                                AxisValueLabel {
                                    // Whole stars only: ten labels crowd
                                    // illegibly at a phone's width, and the
                                    // half-star bars sit between the ones kept.
                                    if let label = value.as(String.self),
                                        !label.contains(".")
                                    {
                                        Text(label).font(.monoUI(9))
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

                // Same rule as the rating chart: nothing finished in the window
                // is an absent chart, not a row of flat bars. The Unknown
                // bucket is rendered whenever it has books in it — an
                // audiobook has no page count, and hiding that would report
                // the distribution over fewer books than were finished.
                if summary.lengthBuckets.contains(where: { $0.books > 0 }) {
                    section("How long they were") {
                        Chart(summary.lengthBuckets) { bucket in
                            BarMark(
                                x: .value("Books", bucket.books),
                                y: .value("Length", bucket.label)
                            )
                            .foregroundStyle(palette.accentColor)
                            .cornerRadius(3)
                        }
                        // Horizontal: the labels are page ranges, which don't
                        // fit under a column but read fine beside a bar.
                        .chartXAxis {
                            AxisMarks { _ in
                                AxisGridLine().foregroundStyle(palette.line2.color)
                                AxisValueLabel().font(.monoUI(9))
                            }
                        }
                        .chartYAxis {
                            AxisMarks(position: .leading) { _ in
                                AxisValueLabel().font(.monoUI(9))
                            }
                        }
                        .frame(height: 132)
                    }
                }

                // Zero-filled to 24 and 7 columns, so an empty period has the
                // same shape as a full one — `hasTimePatterns` is what tells
                // them apart, and without it the section would draw two rows
                // of flat bars and call it a reading pattern. It still shows
                // for a period that is *only* unplaceable activity, because
                // that is the one case a reader needs the note to explain.
                if summary.hasTimePatterns || summary.unzonedSeconds > 0 {
                    section("When you read") {
                        TimePatternCharts(summary: summary)
                    }
                }

                // Omitted rather than emptied: the section's own length is
                // what reports how much the window holds.
                let standouts = Self.standoutRows(summary)
                if !standouts.isEmpty {
                    section("The standouts") {
                        StandoutList(
                            rows: standouts,
                            showFastestReadNote: summary.superlatives.fastestRead != nil
                        )
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

                // Absent until it lands, and absent when the library has been
                // measured for nothing — three zeroes would read as a claim
                // about the collection rather than about the backfill.
                if let librarySize, !librarySize.isEmpty {
                    section("Your library, in reading terms") {
                        LibrarySizeSection(size: librarySize)
                    }
                }

                // Named apart from "How you consumed them" above, which is
                // read-vs-listened seconds. This is the shelf's own mix.
                if let libraryComposition, !libraryComposition.isEmpty {
                    section("What your library is made of") {
                        LibraryCompositionSection(composition: libraryComposition)
                    }
                }

                sessionLogSection
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

    // MARK: - Annual goal

    /// The calendar year the card labels itself with, taken from the server's
    /// `asOfDay` rather than the device clock so the card and the count it
    /// renders can never straddle different years. `nil` on a server too old
    /// to send the day, which also hides the card.
    private func goalYear(_ summary: StatsSummary) -> String? {
        let day = summary.asOfDay
        guard day.count >= 4 else { return nil }
        return String(day.prefix(4))
    }

    private var isOnline: Bool { Connectivity.shared.isOnline }

    /// Progress toward the year's goal, or an invitation to set one — never a
    /// zero-of-zero ring, which reports a goal the reader never made.
    private func goalCard(_ goal: ReadingGoal?, year: String) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            HStack(alignment: .firstTextBaseline) {
                Text("\(year) READING GOAL")
                    .font(.monoUI(10, weight: .medium))
                    .tracking(0.8)
                    .foregroundStyle(palette.accentColor)
                Spacer(minLength: 8)
                Button(goal == nil ? "Set a goal" : "Edit") {
                    goalDraft = goal.map { "\($0.target)" } ?? ""
                    goalError = nil
                    isEditingGoal = true
                }
                .font(.ui(12.5, weight: .medium))
                .foregroundStyle(isOnline ? palette.accentColor : palette.ink3Color)
                .disabled(!isOnline)
            }

            if let goal {
                Text("\(goal.current) of \(goal.target) \(goal.target == 1 ? "book" : "books")")
                    .font(.display(30))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                    .contentTransition(.numericText())

                GoalBar(fraction: goal.fraction, isMet: goal.isMet)
                    .frame(height: 8)

                Text(goal.isMet ? "Goal met" : "\(goal.remaining) to go")
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink2Color)
            } else {
                Text("Set a target for how many books you'd like to finish this year.")
                    .font(.ui(13))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if let goalError {
                Text(goalError)
                    .font(.ui(12))
                    .foregroundStyle(palette.warnColor)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Spacing.lg)
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .fill(palette.bg1Color)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(palette.line2.color, lineWidth: 0.5)
        )
        .animation(Motion.settle, value: goal?.current)
        .screenPadding()
        .alert("Reading goal", isPresented: $isEditingGoal) {
            TextField("Books this year", text: $goalDraft)
                .keyboardType(.numberPad)
            Button("Save") { Task { await saveGoal() } }
            if goal != nil {
                Button("Clear", role: .destructive) { Task { await clearGoal() } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("How many books would you like to finish in \(year)?")
        }
    }

    /// Rule 08 test 1: a goal is account configuration, so this is a direct
    /// call whose failure is surfaced — never a queued op, and the control
    /// that reaches it is disabled while offline.
    private func saveGoal() async {
        guard let target = Int64(goalDraft.trimmingCharacters(in: .whitespaces)),
            target >= 1, target <= 10_000
        else {
            goalError = "Enter a whole number of books between 1 and 10,000."
            return
        }
        await writeGoal(target)
    }

    private func clearGoal() async {
        await writeGoal(nil)
    }

    private func writeGoal(_ target: Int64?) async {
        do {
            let saved = try await UserDataService.setReadingGoal(target: target)
            // Fold the server's answer straight into the rendered summary
            // rather than waiting on the reload, so the bar moves at once.
            summary?.goal = saved
            goalError = nil
            await load(force: true)
        } catch {
            goalError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
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
            // Current before longest: the run you're on is what opens the tab,
            // and the record then reads as context beside it rather than as
            // the headline. The flame follows the live run; the record keeps
            // the trophy.
            StatTile(
                label: "Current streak",
                value: summary.currentStreakDays > 0 ? "\(summary.currentStreakDays)d" : "—",
                icon: "flame"
            )
            StatTile(
                label: "Longest streak",
                value: summary.longestStreakDays > 0 ? "\(summary.longestStreakDays)d" : "—",
                icon: "trophy"
            )
            StatTile(
                label: "Days active",
                value: summary.activeDays > 0 ? "\(summary.activeDays)" : "—",
                icon: "calendar"
            )
            StatTile(
                label: "Pages read",
                value: Self.pagesValue(summary),
                icon: "doc.text"
            )
            // Directly after Pages so the two share a row of the two-column
            // grid: the total says how much, this says how fast, and the pair
            // is the reader's own speed to compare against.
            StatTile(
                label: "Pages an hour",
                value: summary.pagesPerHour.map(Self.rateValue) ?? "—",
                icon: "speedometer"
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

    /// A reading rate for display: one decimal under ten pages an hour, whole
    /// pages above. Mirrors the web drill-in's `rate_value` — nobody reads at
    /// 32.4 pages an hour reproducibly, and the decimal would dress an
    /// estimate as a measurement.
    ///
    /// The branch tests the **rounded** figure, not the raw one: 9.96 at one
    /// decimal is "10.0", which is not "under ten" however it got there.
    private static func rateValue(_ rate: Double) -> String {
        let oneDecimal = (rate * 10).rounded() / 10
        return oneDecimal < 10
            ? String(format: "%.1f", oneDecimal)
            : String(format: "%.0f", rate.rounded())
    }

    /// The Pages read tile's value. Two empty states, not one: a window whose
    /// only activity was listening turned exactly zero pages — audio has no
    /// page analogue, so that is an answer — while anything else unmeasurable
    /// is a genuine em-dash. Kept in step with the web tile's `pages_value`;
    /// the server owns the `audioOnly` distinction so neither surface invents
    /// its own.
    static func pagesValue(_ summary: StatsSummary) -> String {
        if let pages = summary.pagesRead { return "\(pages)" }
        return summary.pagesDetail.audioOnly ? "0" : "\u{2014}"
    }

    /// The cutover caption, or `nil` when the window is fully covered. Page
    /// progress is differenced from stored positions and none exist before the
    /// ledger began, so a window reaching past that day is only partly
    /// measurable and says so rather than reading as complete.
    ///
    /// Gated on the server's own overlap answer, not on the range: a Year
    /// window in the calendar year after the epoch is fully covered and must
    /// not carry the caveat, while a Week window in the days right after it is
    /// not covered and must.
    static func pagesCutoverNote(_ summary: StatsSummary) -> String? {
        guard summary.pagesDetail.predatesLedger,
            let since = summary.pagesDetail.sinceDay
        else { return nil }
        return "Page tracking began \(since); reading before then isn\u{2019}t counted."
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

    /// Every standout the window supports, in the same order as the web card,
    /// so a reader switching surfaces reads the same list.
    ///
    /// The two rankings the web card ends with are deliberately absent: this
    /// tab already renders `topAuthors` / `topTags` in full below, and a
    /// one-line restatement above a whole list is noise.
    static func standoutRows(_ summary: StatsSummary) -> [StandoutRow] {
        let s = summary.superlatives
        let book = { (label: String, sup: BookSuperlative?, detail: (Int64) -> String) in
            sup.map { StandoutRow(label: label, headline: $0.title, detail: detail($0.value)) }
        }
        // Zero seconds means the window has no busiest week, whatever date
        // rode along beside it.
        let busiestWeek = summary.busiestWeekSeconds > 0
            ? summary.busiestWeekStart.map {
                StandoutRow(
                    label: "Busiest week",
                    headline: "Week of \(Self.prettyDay($0))",
                    detail: Format.humanDuration(summary.busiestWeekSeconds)
                )
            }
            : nil
        return [
            book("Longest book", s.longestBook, Self.pagesDetail),
            book("Shortest book", s.shortestBook, Self.pagesDetail),
            book("Fastest read", s.fastestRead, Self.daysDetail),
            book("Longest sitting", s.longestSit, Format.humanDuration),
            s.biggestDay.map {
                StandoutRow(
                    label: "Biggest day",
                    headline: Self.prettyDay($0.day),
                    detail: Format.humanDuration($0.seconds)
                )
            },
            busiestWeek,
        ].compactMap { $0 }
    }

    /// Parses the server's UTC `YYYY-MM-DD`. Fixed-format, so it is pinned to
    /// `en_US_POSIX` and Gregorian — a device on a non-Gregorian calendar
    /// would otherwise fail to read the wire format at all.
    private static let wireDayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    /// Renders "14 Nov 2023". Also pinned to `en_US_POSIX`: the surrounding
    /// copy ("Week of …", "Biggest day") is English, and the device locale
    /// would splice a translated month into it — "Week of 13 nov. 2023".
    private static let displayDayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "d MMM yyyy"
        return f
    }()

    /// A UTC `YYYY-MM-DD` as "14 Nov 2023", passing anything unparseable
    /// through — a malformed day is better company for its figure than none.
    ///
    /// Both formatters are cached statics: `DateFormatter` construction is
    /// expensive and this runs for every standout row on every render.
    static func prettyDay(_ day: String) -> String {
        guard let date = wireDayFormatter.date(from: day) else { return day }
        return displayDayFormatter.string(from: date)
    }

    /// "412 pages" / "1 page" — the unit is the row's, since
    /// `BookSuperlative.value` carries a bare number.
    static func pagesDetail(_ pages: Int64) -> String {
        "\(pages) page\(pages == 1 ? "" : "s")"
    }

    /// "in 3 days" / "in a day" — the server already collapses a same-day
    /// read to 1, so zero never reaches here.
    static func daysDetail(_ days: Int64) -> String {
        days == 1 ? "in a day" : "in \(days) days"
    }

    // MARK: - Session log

    /// The sittings behind every number above — one row per *sit*, not per
    /// heartbeat flush: the server stitches adjacent checkpoint rows before it
    /// pages them.
    @ViewBuilder private var sessionLogSection: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Session log")

            if sessions.isEmpty, let sessionsError {
                // Distinct from the empty state below: "we couldn't load this"
                // and "you have no sittings" are different answers, and only
                // one of them is worth offering a retry for.
                VStack(alignment: .leading, spacing: Spacing.sm) {
                    Text(sessionsError)
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink2Color)
                    Button("Try again") { Task { await loadSessions() } }
                        .font(.monoUI(10.5, weight: .medium))
                        .disabled(sessionsLoading)
                }
            } else if sessions.isEmpty {
                Text("No sittings recorded yet.")
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink3Color)
            } else {
                VStack(spacing: 0) {
                    ForEach(sessions) { entry in
                        NavigationLink(value: Destination.book(uuid: entry.bookUUID)) {
                            SessionLogRow(entry: entry)
                        }
                        .buttonStyle(.plain)
                    }
                }

                // Keyset paging: the cursor names the last row already shown,
                // so a sitting landing mid-scroll can't shift the next page
                // the way an offset would.
                if sessionCursor != nil {
                    Button {
                        Task { await loadMoreSessions() }
                    } label: {
                        Text(sessionsLoading ? "Loading\u{2026}" : "Show more")
                            .font(.monoUI(10.5, weight: .medium))
                            .tracking(0.8)
                            .textCase(.uppercase)
                            .foregroundStyle(palette.ink2Color)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 7)
                            .background(Capsule().fill(palette.bg2Color))
                            .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
                    }
                    .disabled(sessionsLoading)
                }

                if let sessionsError {
                    Text(sessionsError)
                        .font(.ui(12))
                        .foregroundStyle(palette.ink3Color)
                }
            }
        }
        .screenPadding()
    }

    private func loadSessions() async {
        guard !sessionsLoading else { return }
        sessionsLoading = true
        do {
            let page = try await UserDataService.sessionLog()
            sessions = page.entries
            sessionCursor = page.nextBefore
            sessionsError = nil
        } catch {
            sessionsError =
                (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        sessionsLoading = false
    }

    private func loadMoreSessions() async {
        guard let cursor = sessionCursor, !sessionsLoading else { return }
        sessionsLoading = true
        do {
            let page = try await UserDataService.sessionLog(before: cursor)
            sessions.append(contentsOf: page.entries)
            sessionCursor = page.nextBefore
            sessionsError = nil
        } catch {
            sessionsError =
                (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        sessionsLoading = false
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

    /// Best-effort by design: this card is context beside the reader's own
    /// numbers, so a library-size fetch that fails must not take the tab's
    /// error state with it. The cached read lands first and the live one
    /// replaces it, same as `load`.
    private func loadLibrarySize() async {
        do {
            for try await read in UserDataService.librarySize() {
                librarySize = read.value
            }
        } catch {
            // Nothing to say: the section simply doesn't appear.
        }
    }

    /// Best-effort by design, exactly like `loadLibrarySize`.
    private func loadLibraryComposition() async {
        do {
            for try await read in UserDataService.libraryComposition() {
                libraryComposition = read.value
            }
        } catch {
            // Nothing to say: the section simply doesn't appear.
        }
    }

    /// The five panels, in the order they read: what the files are, then what
    /// the books are. Mirrors the web card's `build_panels`.
    static func compositionPanels(_ c: LibraryComposition) -> [CompositionPanel] {
        [
            CompositionPanel(
                title: "Formats", dimension: c.formats,
                // Coverage is always the whole library here (a live book has a
                // file by definition), so the useful disclosure is the overlap.
                note: overlapNote(c.formats),
                empty: "No files indexed yet."),
            CompositionPanel(
                title: "Languages", dimension: c.languages,
                note: coverageNote(c.languages, of: c.books),
                empty: "No language metadata yet."),
            CompositionPanel(
                title: "Publishers", dimension: c.publishers,
                note: coverageNote(c.publishers, of: c.books),
                empty: "No publisher metadata yet."),
            CompositionPanel(
                title: "Published", dimension: c.decades,
                // The uncovered books here are the ones with an absent or
                // unparseable pubdate — unknown, never bucketed into a decade.
                note: coverageNote(c.decades, of: c.books),
                empty: "No publication dates yet."),
            CompositionPanel(
                title: "Genres", dimension: c.genres,
                note: "hand-assigned \u{2014} " + coverageNote(c.genres, of: c.books),
                empty: "No genres assigned yet."),
        ]
    }

    /// "across 58 of 1,510 books" — the denominator, always. A distribution
    /// without its coverage is a guess wearing a chart.
    static func coverageNote(_ dimension: CompositionDimension, of libraryBooks: Int64) -> String {
        coverageLabel(dimension.coverage, of: libraryBooks)
    }

    /// How many books are held in more than one format, and so counted in more
    /// than one bar. Without it the bars simply don't add up to the library.
    static func overlapNote(_ dimension: CompositionDimension) -> String? {
        let overlap = dimension.overlap
        guard overlap > 0 else { return nil }
        return "+\(overlap) \(overlap == 1 ? "book" : "books") held in more than one format"
    }

    /// The footnote for `books` rows whose files are gone. They carry no
    /// format, so they'd otherwise vanish from the bars and leave the counts
    /// failing to reconcile against the library.
    static func ghostedNote(_ ghosted: Int64) -> String? {
        guard ghosted > 0 else { return nil }
        return "\(ghosted) \(ghosted == 1 ? "book" : "books") excluded \u{2014} indexed once, no files on disk now"
    }

    /// The library figures worth rendering, skipping anything nothing has
    /// been measured for — a "0 words" row describes a library that doesn't
    /// exist. Mirrors the web card's `build_figures`.
    static func libraryFigures(_ size: LibrarySize) -> [LibraryFigure] {
        var figures: [LibraryFigure] = []
        if !size.words.isEmpty {
            figures.append(
                LibraryFigure(
                    value: compactCount(size.words.total),
                    unit: "words",
                    coverage: coverageLabel(size.words, of: size.books)
                ))
        }
        if !size.pages.isEmpty {
            figures.append(
                LibraryFigure(
                    value: compactCount(size.pages.total),
                    unit: "est. pages",
                    coverage: coverageLabel(size.pages, of: size.books)
                ))
        }
        if !size.listeningSeconds.isEmpty {
            let (value, unit) = audioValue(size.listeningSeconds.total)
            figures.append(
                LibraryFigure(
                    value: value,
                    unit: unit,
                    coverage: coverageLabel(size.listeningSeconds, of: size.books)
                ))
        }
        return figures
    }

    /// A large count in the form a reader can hold — "412M", "1.6M", "94.2K",
    /// "812". Nobody needs the last four digits of a word count.
    static func compactCount(_ n: Int64) -> String {
        let v = Double(n)
        // Each tier opens at 999.5 of the one below rather than at a clean
        // power of ten: 999_999 rounds to 1000 at "K", so it has to render as
        // "1.0M". Mirrors `compact` in frontend/src/pages/stats/library.rs.
        for (limit, div, suffix) in [(999.5e6, 1e9, "B"), (999.5e3, 1e6, "M"), (1e4, 1e3, "K")] {
            if v >= limit {
                let scaled = v / div
                return String(format: scaled < 100 ? "%.1f\(suffix)" : "%.0f\(suffix)", scaled)
            }
        }
        return "\(n)"
    }

    /// Audio length in the unit that fits it: hours below a week, days beyond.
    /// "94 days of audio" is the sentence this section exists to let a reader
    /// say; 2,256 hours is the same fact nobody can picture.
    static func audioValue(_ seconds: Int64) -> (String, String) {
        let hours = Double(seconds) / 3600
        // Round first, then pick the unit off the rounded figure: branching on
        // the raw hours renders 1h40m as "2 hour", and promotes to days only
        // after the hours reading has already rounded to 168.
        let wholeHours = Int64(hours.rounded())
        if wholeHours < 168 {
            return ("\(wholeHours)", wholeHours == 1 ? "hour" : "hours")
        }
        return (String(format: "%.0f", hours / 24), "days")
    }

    /// "across 1,204 of 1,510 books" — the denominator, always. A figure
    /// without it is a guess wearing a number.
    static func coverageLabel(_ measured: MeasuredTotal, of libraryBooks: Int64) -> String {
        "across \(groupedCount(measured.books)) of \(groupedCount(libraryBooks)) books"
    }

    /// A count with its thousands separators. Shared so a bar's own number and
    /// the coverage line beneath it can't render the same figure two ways.
    static func groupedCount(_ n: Int64) -> String {
        NumberFormatter.localizedString(from: NSNumber(value: n), number: .decimal)
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

/// One library-scale figure: the total, its unit, and the coverage behind it.
struct LibraryFigure: Identifiable, Hashable {
    let value: String
    let unit: String
    let coverage: String

    var id: String { unit }
}

private struct LibrarySizeSection: View {
    let size: LibrarySize

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            ForEach(StatsView.libraryFigures(size)) { figure in
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(figure.value)
                            .font(.display(28))
                            .foregroundStyle(palette.ink0Color)
                        Text(figure.unit)
                            .font(.ui(13))
                            .foregroundStyle(palette.ink2Color)
                    }
                    Text(figure.coverage)
                        .font(.monoUI(11))
                        .foregroundStyle(palette.ink3Color)
                }
                .accessibilityElement(children: .combine)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The annual goal's progress bar. Its own view so the width animation is
/// driven by `fraction` alone rather than by every summary field changing.
private struct GoalBar: View {
    let fraction: Double
    let isMet: Bool

    @Environment(\.palette) private var palette

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(palette.bg2Color)
                Capsule()
                    .fill(isMet ? palette.okColor : palette.accentColor)
                    .frame(width: max(0, geometry.size.width * fraction))
            }
        }
        .animation(Motion.settle, value: fraction)
        .accessibilityElement()
        .accessibilityLabel("Reading goal progress")
        .accessibilityValue("\(Int((fraction * 100).rounded())) percent")
    }
}

/// One rendered dimension: its heading, its bars, and the line beneath them
/// that says what the bars can't speak for.
struct CompositionPanel: Identifiable, Hashable {
    let title: String
    let dimension: CompositionDimension
    let note: String?
    let empty: String

    var id: String { title }
}

private struct LibraryCompositionSection: View {
    let composition: LibraryComposition
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            ForEach(StatsView.compositionPanels(composition)) { panel in
                CompositionPanelView(panel: panel)
            }
            if let ghosted = StatsView.ghostedNote(composition.ghostedBooks) {
                Text(ghosted)
                    .font(.monoUI(11))
                    .foregroundStyle(palette.ink3Color)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// One dimension's bars, or its empty state. A dimension nothing in the
/// library carries renders a sentence rather than an axis with no bars on it.
private struct CompositionPanelView: View {
    let panel: CompositionPanel

    @Environment(\.palette) private var palette

    /// Scaled to the tallest bar rather than to the library total: a histogram
    /// whose bars are all four points wide has drawn the shape out of itself.
    private var peak: Int64 { panel.dimension.slices.map(\.books).max() ?? 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(panel.title)
                .font(.display(15))
                .foregroundStyle(palette.ink1Color)
            if panel.dimension.slices.isEmpty {
                Text(panel.empty)
                    .font(.ui(13))
                    .foregroundStyle(palette.ink3Color)
            } else {
                ForEach(panel.dimension.slices) { slice in
                    CompositionBar(slice: slice, peak: peak)
                }
                if let note = panel.note {
                    Text(note)
                        .font(.monoUI(11))
                        .foregroundStyle(palette.ink3Color)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct CompositionBar: View {
    let slice: CompositionSlice
    let peak: Int64

    @Environment(\.palette) private var palette

    private var fraction: Double {
        guard peak > 0 else { return 0 }
        return min(1, Double(slice.books) / Double(peak))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(slice.label)
                    .font(.ui(14))
                    .foregroundStyle(palette.ink1Color)
                    .lineLimit(1)
                Spacer(minLength: Spacing.sm)
                // The count, not the share: "48 books" answers the question a
                // reader brought to a composition chart. Grouped like the
                // coverage line, so a four-digit bucket doesn't read
                // differently from its own note.
                Text(StatsView.groupedCount(slice.books))
                    .font(.monoUI(12))
                    .foregroundStyle(palette.ink2Color)
            }
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(palette.bg2Color)
                    Capsule()
                        .fill(palette.accentColor)
                        .frame(width: geometry.size.width * fraction)
                }
            }
            .frame(height: 8)
        }
        .accessibilityElement(children: .combine)
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

/// One standout: what it measures, what won, and by how much. Built by
/// `StatsView.standoutRows`, which is where the units live.
struct StandoutRow: Identifiable, Hashable {
    let label: String
    let headline: String
    let detail: String

    var id: String { label }
}

private struct StandoutList: View {
    let rows: [StandoutRow]
    let showFastestReadNote: Bool

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            ForEach(rows) { row in
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.label)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(row.headline)
                            .font(.display(17))
                            .foregroundStyle(palette.ink0Color)
                            .lineLimit(1)
                            .minimumScaleFactor(0.7)
                        Spacer(minLength: 0)
                        Text(row.detail)
                            .font(.monoUI(11.5))
                            .foregroundStyle(palette.ink2Color)
                            .layoutPriority(1)
                    }
                }
                .accessibilityElement(children: .combine)
            }
            if showFastestReadNote {
                // The floor is part of the claim, not an aside: without it a
                // book read mostly on another device reads as a sprint. The
                // lower-bound clause mirrors the web note — `shared`'s
                // `fastest_read` doc requires every surface to state it.
                Text(
                    "Fastest read counts days from your first tracked session, over books with "
                        + "at least \(Format.humanDuration(Superlatives.fastestReadMinSeconds)) "
                        + "of recorded time — reading done before tracking, or on a device that "
                        + "reports nothing, can only make a book look faster than it was."
                )
                .font(.ui(11))
                .foregroundStyle(palette.ink3Color)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
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

/// One sitting: when it started, what it was, and how long it ran.
private struct SessionLogRow: View {
    let entry: SessionLogEntry

    @Environment(\.palette) private var palette

    /// Device-local, unlike the web log's UTC: this is a native app on a
    /// reader's own phone, and a sitting they remember starting at 9pm should
    /// say so.
    private var when: String {
        Date(timeIntervalSince1970: TimeInterval(entry.startedAt))
            .formatted(date: .abbreviated, time: .shortened)
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(entry.title)
                    .font(.ui(13.5, weight: .medium))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                Text("\(when) \u{b7} \(entry.format.label)")
                    .font(.monoUI(10))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(Format.humanDuration(entry.seconds))
                .font(.ui(12.5))
                .foregroundStyle(palette.ink2Color)
        }
        .padding(.vertical, 9)
        .overlay(alignment: .top) {
            Rectangle().fill(palette.line2.color).frame(height: 0.5)
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}
