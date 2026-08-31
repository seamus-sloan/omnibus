//  StatsView.swift
//  Reading stats, in two bands.
//
//  The period switcher governs only part of this screen, and the screen says
//  so. Windowed figures live under one boundary — the accent "In this window"
//  header, whose control pins while you are inside it and releases the moment
//  the standing rule reaches the top. Everything else — the streak, the goals,
//  the heatmap, the trailing twelve months, the library's own scale — sits
//  outside it, because `shared/src/stats.rs` says those fields are not
//  windowed and a page that interleaved the two left a reader unable to tell
//  what a period switch would move.

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
    /// What is open right now. Its own read for the third time over: being
    /// mid-book is a standing fact, so it belongs below the standing rule and
    /// must not reload on a period switch.
    @State private var resumePoints: [ResumePoint] = []
    /// The all-time summary, held only for the two surfaces drawn off
    /// `heatmap` — the activity grid and the four-week strip.
    ///
    /// `db::stats::compute` scopes the heatmap to the *window's* start, so on
    /// Week the payload carries fourteen days and the grid a reader is told is
    /// "not tied to the window" would visibly empty out on a period switch.
    /// Reading those two off the widest window instead is what makes the claim
    /// true. Cached like `librarySize`, so it costs a replica read after the
    /// first fetch — and it is the same entry the All pill warms anyway.
    @State private var standingSummary: StatsSummary?
    @State private var isLoading = true
    @State private var error: String?
    @State private var goalEditor: GoalEditorTarget?
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
            await loadStandingSummary()
            await loadResumePoints()
            await loadSessions()
        }
        .onChange(of: range) { _, _ in Task { await load() } }
        .sheet(item: $goalEditor) { target in
            if let summary {
                GoalEditorSheet(
                    target: target,
                    summary: summary,
                    // Folded straight into the rendered summary rather than
                    // waiting on the reload, so the ring moves at once.
                    onDailySaved: { goals in
                        self.summary?.dailyGoals = goals
                        Task { await load(force: true) }
                    },
                    onAnnualSaved: { goal in
                        self.summary?.goal = goal
                        Task { await load(force: true) }
                    }
                )
            }
        }
    }

    // MARK: - The screen

    private func content(_ summary: StatsSummary) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 30, pinnedViews: [.sectionHeaders]) {
                // The masthead reserves its own bottom margin, which the
                // stack's spacing would then double.
                Masthead(title: "Stats")
                    .padding(.bottom, -Spacing.lg)

                StreakHeadline(summary: summary)
                DailyGoalsCard(summary: summary) { goalEditor = .daily }
                if let year = Self.goalYear(summary) {
                    YearGoalCard(summary: summary, year: year) {
                        goalEditor = .annual(year: year)
                    }
                }
                // Both off the unwindowed summary — see `standingSummary`.
                let unwindowed = standingSummary ?? summary
                LastFourWeeksCard(summary: unwindowed)

                if !unwindowed.heatmap.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        StatsSectionLabel("Activity").screenPadding()
                        HeatmapView(days: unwindowed.heatmap, asOf: unwindowed.asOfDay)
                    }
                }

                Section {
                    windowed(summary)
                } header: {
                    WindowBandHeader(range: $range, caption: Self.rangeCaption(summary))
                }

                // Its own section, and that is the whole mechanism: a pinned
                // header is displaced by the *next* one, so the standing rule
                // being a header is what makes the period control release
                // exactly as the rule reaches the top. Left as loose content
                // the control stayed pinned over the sections it does not
                // govern, which is the one thing this layout must not do.
                Section {
                    standing(summary)
                } header: {
                    StandingBandHeader()
                }
            }
            .padding(.bottom, 40)
        }
        .scrollIndicators(.hidden)
    }

    /// Everything the control above it governs.
    @ViewBuilder
    private func windowed(_ summary: StatsSummary) -> some View {
        WindowHeadline(summary: summary, eyebrow: Self.windowLabel(range))
        tiles(summary)

        if let note = Self.pagesCutoverNote(summary) {
            Text(note)
                .font(.footnote)
                .foregroundStyle(palette.ink3Color)
                .screenPadding()
        }

        if summary.hasTimePatterns || summary.unzonedSeconds > 0 {
            StatsSection("When you read") {
                StatsCard { ReadingClock(summary: summary) }
            }
        }

        if !summary.genreShare.isEmpty {
            StatsSection("How you spent it") { GenreDonut(summary: summary) }
        }

        // Omitted rather than emptied: the section's own length is what
        // reports how much the window holds.
        let standouts = Self.standoutRows(summary)
        if !standouts.isEmpty {
            StatsSection("The standouts") {
                StandoutsCard(
                    rows: standouts,
                    showFastestReadNote: summary.superlatives.fastestRead != nil
                )
            }
        }

        // Kept from the previous tab and left inside this band because every
        // one of them is windowed: they are the drill-in the redesign's tiles
        // summarise, not standing figures.
        RatingDistribution(buckets: summary.ratingHistogram)
        LengthDistribution(buckets: summary.lengthBuckets)

        if !summary.topAuthors.isEmpty {
            StatsSection("Top authors") {
                RankedList(entries: summary.topAuthors) { .searchResults(query: $0.name) }
            }
        }
        if !summary.topTags.isEmpty {
            StatsSection("Top tags") {
                RankedList(entries: summary.topTags) { .tag(name: $0.name) }
            }
        }
        if !summary.finishedBooks.isEmpty {
            FinishedRail(books: summary.finishedBooks)
        }
    }

    /// Everything it does not. The absence of the accent boundary is the
    /// signal, and the rule above says it in words.
    @ViewBuilder
    private func standing(_ summary: StatsSummary) -> some View {
        if !resumePoints.isEmpty {
            StatsSection("In progress") { InProgressCard(points: resumePoints) }
        }

        if !summary.booksPerMonth.isEmpty {
            StatsSection("Books finished") { TrailingYearCard(months: summary.booksPerMonth) }
        }

        // Absent until it lands, and absent when the library has been measured
        // for nothing — three zeroes would read as a claim about the
        // collection rather than about the backfill.
        if let librarySize, !librarySize.isEmpty {
            StatsSection("Your library, in reading terms") {
                LibrarySizeSection(size: librarySize)
            }
        }
        // Named apart from "How you spent it" above, which is the reader's own
        // genre mix in the window. This is the shelf's own make-up.
        if let libraryComposition, !libraryComposition.isEmpty {
            StatsSection("What your library is made of", spacing: 18) {
                LibraryCompositionSection(composition: libraryComposition)
            }
        }

        sessionLogSection
    }

    private func tiles(_ summary: StatsSummary) -> some View {
        // Deltas are suppressed on Lifetime: there is no window before all of
        // them, and `previous` is zeroed there rather than absent.
        let comparable = range != .allTime
        return LazyVGrid(
            columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)],
            spacing: 12
        ) {
            WindowTile(
                label: "Books finished",
                value: "\(summary.booksFinished)",
                icon: "checkmark.circle",
                delta: comparable
                    ? StatsFormat.delta(summary.booksFinished, summary.previous.booksFinished)
                    : nil
            )
            WindowTile(
                label: "Days active",
                value: summary.activeDays > 0 ? "\(summary.activeDays)" : "\u{2014}",
                icon: "calendar"
            )
            WindowTile(
                label: "Pages read",
                value: Self.pagesValue(summary),
                icon: "doc.text",
                delta: comparable
                    ? StatsFormat.percentDelta(summary.pagesRead ?? 0, summary.previous.pagesRead)
                    : nil
            )
            // Directly after Pages so the two share a row: the total says how
            // much, this says how fast, and the pair is the reader's own speed
            // to compare against.
            WindowTile(
                label: "Pages an hour",
                value: summary.pagesPerHour.map(Self.rateValue) ?? "\u{2014}",
                icon: "speedometer"
            )
            WindowTile(
                label: "Avg rating",
                value: summary.avgStars.map { String(format: "%.1f", $0) } ?? "\u{2014}",
                icon: "star"
            )
            WindowTile(
                label: "Books open",
                value: summary.booksActive > 0 ? "\(summary.booksActive)" : "\u{2014}",
                icon: "book"
            )
        }
        .screenPadding()
    }

    // MARK: - Window labels

    /// What the band's eyebrow calls the period. `StatsRange.label` is the
    /// control's own short form ("Week"); the headline wants the sentence
    /// form, and "Lifetime" is already one.
    static func windowLabel(_ range: StatsRange) -> String {
        switch range {
        case .week: "This week"
        case .month: "This month"
        case .year: "This year"
        case .allTime: "All time"
        }
    }

    /// The window the control currently names, spelled out beside it — a
    /// switcher that says "Week" without saying *which* week is a control
    /// whose effect you have to infer from the figures moving.
    ///
    /// Every bound is read off `asOfDay`, the server's own day, and matches
    /// `window_start_expr` in `db/src/stats/compute.rs`: Week is a rolling
    /// seven days ending today, not a calendar week.
    static func rangeCaption(_ summary: StatsSummary) -> String {
        guard let asOf = StatsFormat.wireDay.date(from: summary.asOfDay) else {
            return windowLabel(summary.range)
        }
        switch summary.range {
        case .week:
            guard let start = StatsFormat.utc.date(byAdding: .day, value: -6, to: asOf) else {
                return "Last 7 days"
            }
            return "Week of \(StatsFormat.day(start, "d MMM"))"
        case .month:
            return StatsFormat.day(asOf, "MMMM yyyy")
        case .year:
            return "\(StatsFormat.day(asOf, "yyyy")) to date"
        case .allTime:
            return "Everything recorded"
        }
    }

    /// The calendar year the goal card labels itself with, taken from the
    /// server's `asOfDay` rather than the device clock so the card and the
    /// count it renders can never straddle different years. `nil` on a server
    /// too old to send the day, which also hides the card.
    static func goalYear(_ summary: StatsSummary) -> String? {
        let day = summary.asOfDay
        guard day.count >= 4 else { return nil }
        return String(day.prefix(4))
    }

    // MARK: - Tile values

    /// A reading rate for display: one decimal under ten pages an hour, whole
    /// pages above. Mirrors the web drill-in's `rate_value` — nobody reads at
    /// 32.4 pages an hour reproducibly, and the decimal would dress an
    /// estimate as a measurement.
    ///
    /// The branch tests the **rounded** figure, not the raw one: 9.96 at one
    /// decimal is "10.0", which is not "under ten" however it got there.
    static func rateValue(_ rate: Double) -> String {
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

    // MARK: - Standouts

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

    /// A UTC `YYYY-MM-DD` as "14 Nov 2023", passing anything unparseable
    /// through — a malformed day is better company for its figure than none.
    static func prettyDay(_ day: String) -> String {
        guard let date = StatsFormat.wireDay.date(from: day) else { return day }
        return StatsFormat.day(date, "d MMM yyyy")
    }

    /// "412 pages" / "1 page" — the unit is the row's, since
    /// `BookSuperlative.value` carries a bare number.
    static func pagesDetail(_ pages: Int64) -> String {
        StatsFormat.counted(pages, "page")
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
            StatsSectionLabel("Session log")

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

    // MARK: - Loading

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

    /// The widest window, for the heatmap alone. Best-effort: the two strips
    /// fall back to the rendered summary's own heatmap if it never lands.
    private func loadStandingSummary() async {
        do {
            for try await read in UserDataService.stats(range: .allTime) {
                standingSummary = read.value
            }
        } catch {
            // Nothing to say: the grid falls back to the window's own days.
        }
    }

    /// Best-effort by design, exactly like `loadLibrarySize`.
    private func loadResumePoints() async {
        do {
            for try await read in UserDataService.recentProgress() {
                resumePoints = read.value
            }
        } catch {
            // Nothing to say: the section simply doesn't appear.
        }
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
}

/// One standout: what it measures, what won, and by how much. Built by
/// `StatsView.standoutRows`, which is where the units live.
struct StandoutRow: Identifiable, Hashable {
    let label: String
    let headline: String
    let detail: String

    var id: String { label }
}
