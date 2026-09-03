//  BookDetailView.swift
//  The book detail: full-bleed cover art running off the top edge and the
//  page opening on it whole — the cover, or the generated plate at the same
//  size — with the panel resting under it. Past that, the layout follows the
//  reader's scroll-stops preference. Off (the default), the flow: every
//  section — Home · Stats · Highlights · Journals · The files · More —
//  runs on in one continuous list, nothing capped. On, the marquee: the
//  same sections as six snap stops on a vertical pager, one screenful
//  each. Chrome is immersive either way: glass discs over the
//  art, a persistent bottom action bar so Resume never scrolls away, and
//  the marquee's tappable dot rail or the flow's nav strip behind the discs.

import SwiftUI

@Observable
@MainActor
final class BookDetailModel {
    var book: Book?
    var rating: Double = 0
    var otherRatings: [AttributedRating] = []
    var readStatus: ReadStatus = .unread
    var journals: [JournalEntry] = []
    var highlights: [Highlight] = []
    var bookmarks: [Bookmark] = []
    var shelvesContaining: Set<Int64> = []
    /// Every shelf the user has, for naming the membership chips.
    var allShelves: [ShelfSummary] = []
    var wishlistEntry: WishlistEntry?
    /// Cross-format sync state for the files stop; `nil` while unknown
    /// (offline, or the fetch hasn't landed). Network-only read.
    var syncState: CrossFormatResumeState?
    /// Saved positions, one per format. What the Home ruler and the Resume
    /// label are drawn from.
    var epubProgress: ProgressRecord?
    var audioProgress: ProgressRecord?
    /// Whole-book audio length from the manifest, for turning an audio
    /// position into a fraction. `nil` until fetched, and for HLS manifests.
    var audioDuration: Double?
    /// This reader's sittings of this book, newest first — the stats stop.
    var sessions: [SessionLogEntry] = []
    /// The rest of the series, in reading order, when the book belongs to one.
    var seriesBooks: [Book] = []
    /// Other books by the same first author — the shelf and recommendation
    /// strips.
    var authorBooks: [Book] = []
    var suggestions: [SuggestedBook] = []
    var isLoading = true
    var error: String?

    /// Guards the related fetches (series, author, manifest) against running
    /// concurrently; a fetch that came back empty is retried on the next book
    /// emission or refresh rather than skipped forever.
    private var relatedInFlight = false

    /// Every section is its own live read, so each paints from the replica at
    /// once and updates independently if the server disagrees. Nothing here
    /// waits on anything else — a slow endpoint can no longer hold the page.
    func load(uuid: String) async {
        isLoading = book == nil

        await withTaskGroup(of: Void.self) { group in
            group.addTask { @MainActor in
                do {
                    for try await read in LibraryService.book(uuid: uuid) {
                        self.book = read.value
                        self.error = nil
                        self.isLoading = false
                        self.loadRelated(read.value)
                    }
                } catch {
                    if self.book == nil {
                        self.error = (error as? APIError)?.errorDescription
                            ?? error.localizedDescription
                    }
                }
                self.isLoading = false
            }
            group.addTask { @MainActor in
                self.syncState =
                    (try? await UserDataService.crossFormatResume(uuid: uuid, target: .audio))?
                        .state
            }
            group.addTask { @MainActor in
                for await record in UserDataService.progress(uuid: uuid, format: .epub).values() {
                    self.epubProgress = record
                }
            }
            group.addTask { @MainActor in
                for await record in UserDataService.progress(uuid: uuid, format: .audio).values() {
                    self.audioProgress = record
                }
            }
            group.addTask { @MainActor in
                await self.loadSessions(uuid: uuid)
            }
            group.addTask { @MainActor in
                for await record in UserDataService.rating(uuid: uuid).values() {
                    self.rating = record.stars
                }
            }
            group.addTask { @MainActor in
                for await others in UserDataService.otherRatings(uuid: uuid).values() {
                    self.otherRatings = others
                }
            }
            group.addTask { @MainActor in
                for await record in UserDataService.readStatus(uuid: uuid).values() {
                    self.readStatus = record.status
                }
            }
            group.addTask { @MainActor in
                for await entries in UserDataService.journals(uuid: uuid).values() {
                    self.journals = entries
                }
            }
            group.addTask { @MainActor in
                for await items in UserDataService.highlights(uuid: uuid).values() {
                    // Sorted once at ingestion: the stops re-render on every
                    // scroll tick, so a per-render sort is a real cost the
                    // flow's uncapped list would pay constantly.
                    self.highlights = items.sorted { $0.createdAt > $1.createdAt }
                }
            }
            group.addTask { @MainActor in
                for await items in UserDataService.bookmarks(uuid: uuid).values() {
                    self.bookmarks = items
                }
            }
            group.addTask { @MainActor in
                for await ids in UserDataService.shelvesContaining(uuid: uuid).values() {
                    self.shelvesContaining = Set(ids)
                }
            }
            group.addTask { @MainActor in
                for await shelves in UserDataService.shelves().values() {
                    self.allShelves = shelves
                }
            }
            group.addTask { @MainActor in
                for await entry in UserDataService.wishlistEntry(uuid: uuid).values() {
                    self.wishlistEntry = entry
                }
            }
            group.addTask { @MainActor in
                for await items in LibraryService.suggestions(uuid: uuid).values() {
                    self.suggestions = items
                }
            }
        }
    }

    /// Fetches keyed on facts only the book record carries — the series, the
    /// first author, whether there is audio to measure. Each leg runs only
    /// while its answer is still missing, so a transient failure is retried
    /// on the next book emission or pull-to-refresh instead of being skipped
    /// for the life of the model.
    private func loadRelated(_ book: Book) {
        guard !relatedInFlight else { return }

        let seriesId = seriesBooks.isEmpty ? book.seriesId : nil
        let authorId = authorBooks.isEmpty ? book.creators.first?.id : nil
        let wantsDuration = book.hasAudiobook && audioDuration == nil
        guard seriesId != nil || authorId != nil || wantsDuration else { return }

        relatedInFlight = true
        Task { @MainActor in
            defer { relatedInFlight = false }
            await withTaskGroup(of: Void.self) { group in
                if let seriesId {
                    group.addTask { @MainActor in
                        for await detail in LibraryService.series(id: seriesId).values() {
                            self.seriesBooks = detail.books
                        }
                    }
                }
                if let authorId {
                    group.addTask { @MainActor in
                        for await detail in LibraryService.author(id: authorId).values() {
                            self.authorBooks = detail.books.filter { $0.id != book.id }
                        }
                    }
                }
                if wantsDuration {
                    group.addTask { @MainActor in
                        self.audioDuration =
                            (try? await LibraryService.audiobookManifest(uuid: book.uuid))?
                                .totalDuration
                    }
                }
            }
        }
    }

    /// Pages the whole per-book session log. A single book's log is small —
    /// dozens of sittings, not thousands — but the cap keeps a pathological
    /// one from looping forever. Committed only after every page landed: a
    /// failed page must not publish a partial accumulator as the complete
    /// history, wiping stats a previous load showed.
    private func loadSessions(uuid: String) async {
        var entries: [SessionLogEntry] = []
        var before: String?
        for _ in 0..<12 {
            guard let page = try? await UserDataService.sessionLog(book: uuid, before: before)
            else { return }
            entries += page.entries
            guard let next = page.nextBefore else { break }
            before = next
        }
        sessions = entries
    }

    func setRating(_ stars: Double, uuid: String) async {
        rating = stars
        await UserDataService.setRating(uuid: uuid, stars: stars)
    }

    func clearRating(uuid: String) async {
        rating = 0
        await UserDataService.clearRating(uuid: uuid)
    }

    func setStatus(_ status: ReadStatus, uuid: String) async {
        readStatus = status
        await UserDataService.setReadStatus(uuid: uuid, status: status)
    }

    /// A wishlisted book the library holds no files for. The ruler and Resume
    /// drop out; the action bar becomes Find a copy · check in.
    var isWishlistOnly: Bool {
        guard let book else { return false }
        return wishlistEntry != nil && !book.hasEbook && !book.hasAudiobook
    }
}

/// The six stops, in scroll order. The trailing More folds what used to be
/// two stops — the shelf block and the recommendations — into one, matching
/// the design's six-section list on both clients.
enum DetailStop: Int, CaseIterable, Identifiable {
    case home, stats, highlights, journals, files, more

    var id: Int { rawValue }

    var name: String {
        switch self {
        case .home: "Home"
        case .stats: "Stats"
        case .highlights: "Highlights"
        case .journals: "Journals"
        case .files: "The files"
        case .more: "More"
        }
    }
}

struct BookDetailView: View {
    let uuid: String

    init(uuid: String) {
        self.uuid = uuid
    }

    /// Window the artwork pans inside once the Home panel has lifted.
    static let artHeight: CGFloat = 470
    /// Clearance the panel keeps for the action bar.
    static let barClearance: CGFloat = 108
    /// Scroll id of the rest run above the Home stop — the pre-Home snap
    /// target the two-position panel rests at.
    static let restMarkerID = -1
    /// Scroll id of the flow body's snap position, `DetailRead.flowNavPeek`
    /// short of the cover's end — where the lift cue's tap sends the list.
    static let flowLiftedMarkerID = -2

    /// What the journal composer was opened for. `Identifiable` so the
    /// composer can be an item-driven sheet, which is what keeps a chosen
    /// entry attached to the presentation that carries it.
    enum ComposerTarget: Identifiable {
        case new
        case editing(JournalEntry)

        var id: String {
            switch self {
            case .new: "new"
            case .editing(let entry): entry.pathID
            }
        }

        /// The entry being edited, or `nil` when writing a new one.
        var entry: JournalEntry? {
            if case .editing(let entry) = self { return entry }
            return nil
        }
    }

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @Environment(AppState.self) private var app
    @Environment(AudioPlayer.self) private var player
    @Environment(\.bookZoomNamespace) private var bookZoom
    @State private var model = BookDetailModel()
    @State private var showShelfPicker = false
    @State private var showAlignment = false
    @State private var showBookmarks = false
    @State private var showAudioFilePicker = false
    @State private var showCheckIn = false
    @State private var showDescription = false
    @State private var showAllHighlights = false
    @State private var showAllJournals = false
    /// The entry open in the journal drawer.
    @State private var openJournal: JournalEntry?
    /// An entry picked inside the all-entries sheet; opened in the drawer
    /// once that sheet has dismissed — two sheets can't be up at once.
    @State private var pendingJournal: JournalEntry?
    /// The passage being turned into a card, driving the quote sheet.
    @State private var quoteTarget: QuoteRequest?
    /// A line picked inside the all-highlights sheet; made into a card once
    /// that sheet has dismissed, for the same reason as `pendingJournal`.
    @State private var pendingQuote: Highlight?
    /// The file chosen in the picker, opened from the sheet's `onDismiss` —
    /// presenting the full-screen player while the sheet is still up would
    /// be refused.
    @State private var pickedAudioFile: BookFileInfo?
    /// What the journal composer is open on, when it is. The entry rides
    /// with the presentation rather than in a payload `@State` beside a Bool:
    /// `.sheet(isPresented:)` captured its content before that payload landed,
    /// so Edit opened a blank "New entry".
    @State private var composing: ComposerTarget?
    /// Continuous page position, 0...6 — drives the art pan and fade.
    @State private var page: CGFloat = 0
    /// The Home panel's rest→lifted progress, 0...1 — drives the art's
    /// whole→windowed transition and the panel's padding and tint.
    @State private var lift: CGFloat = 0
    /// Whether the Home panel has settled lifted — the discrete switch the
    /// handle label, tag row, and description clamp key on.
    @State private var lifted = false
    /// The stop the scroller has settled nearest to.
    @State private var at: Int = 0
    /// Flow only: whether the cover has scrolled away — the discs go flat
    /// and the nav strip fades in under them.
    @State private var flowPast = false
    /// Scroll-position binding for the dot rail's jumps. Written on a tap;
    /// the scroller keeps it updated as pages snap.
    @State private var scrollTarget: Int?
    /// Whether the screen has appeared before. The first `onAppear` belongs
    /// to `.task`'s load; every later one means a pushed screen — the
    /// metadata editor, a nested detail — was popped off, and whatever it
    /// wrote should show immediately.
    @State private var hasAppeared = false

    private var downloads = DownloadManager.shared
    private var presentation = Presentation.shared

    var body: some View {
        Group {
            if model.isLoading {
                LoadingView()
            } else if let book = model.book {
                content(book)
            } else if let error = model.error {
                ErrorStateView(message: error) { Task { await model.load(uuid: uuid) } }
            }
        }
        .background(ScreenBackground())
        // The screen carries its own chrome — glass discs over the art — so
        // the system bar goes away entirely rather than floating over it.
        // The tab bar needs hiding *here* too: `MainTabView` only suppresses
        // its custom bar, and with the safe-area inset empty the native
        // floating pill re-materializes on this screen without this.
        .toolbar(.hidden, for: .navigationBar)
        .toolbar(.hidden, for: .tabBar)
        .navigationBarBackButtonHidden(true)
        // Hiding the bar kills the system's edge-swipe pop; bring it back.
        .keepsEdgeSwipeBack()
        .bookZoomDestination(uuid, in: bookZoom)
        .task { await model.load(uuid: uuid) }
        .refreshTask { await model.load(uuid: uuid) }
        // Refetch on the way back from a pushed screen (edited metadata, a
        // nested detail). Sheets reload through their own callbacks; the
        // reader and player are full-screen covers over the tab view, so
        // closing one never re-appears this view — that path is the
        // `progressToken` observation below.
        .onAppear {
            guard hasAppeared else {
                hasAppeared = true
                return
            }
            Task { await model.load(uuid: uuid) }
        }
        // Refetch once a reading or listening session has *persisted* its
        // final position — the position, status, highlights, and stats it
        // wrote should show the moment the reader or player closes. Keyed on
        // the token rather than the dismissal so the read never lands before
        // the write it is meant to pick up.
        .onChange(of: presentation.progressToken) { _, _ in
            Task { await model.load(uuid: uuid) }
        }
        .sheet(isPresented: $showAlignment) {
            if let book = model.book {
                AlignmentSheet(book: book) {
                    Task { await model.load(uuid: uuid) }
                }
            }
        }
        .sheet(item: $composing) { target in
            if let book = model.book {
                JournalComposer(book: book, editing: target.entry) {
                    Task { await model.load(uuid: uuid) }
                }
            }
        }
        .sheet(isPresented: $showShelfPicker, onDismiss: {
            Task {
                for await ids in UserDataService.shelvesContaining(uuid: uuid).values() {
                    model.shelvesContaining = Set(ids)
                }
            }
        }) {
            if let book = model.book {
                ShelfPickerSheet(book: book)
            }
        }
        .sheet(isPresented: $showBookmarks) {
            if let book = model.book {
                BookmarksSheet(book: book, isAudio: book.hasAudiobook)
            }
        }
        .sheet(isPresented: $showAudioFilePicker, onDismiss: openPickedAudioFile) {
            if let book = model.book {
                AudioFilePickerSheet(book: book) { file in
                    pickedAudioFile = file
                }
            }
        }
        .sheet(isPresented: $showCheckIn) { CheckInView() }
        .sheet(isPresented: $showDescription) {
            if let book = model.book {
                DescriptionDrawer(book: book)
            }
        }
        .sheet(isPresented: $showAllHighlights, onDismiss: {
            guard let pending = pendingQuote else { return }
            pendingQuote = nil
            quote(pending)
        }) {
            if let book = model.book {
                AllHighlightsSheet(book: book, highlights: model.highlights) { highlight in
                    pendingQuote = highlight
                    showAllHighlights = false
                }
            }
        }
        .sheet(item: $quoteTarget) { request in
            if let book = model.book {
                QuoteCardSheet(quote: request.text, book: book)
            }
        }
        .sheet(isPresented: $showAllJournals, onDismiss: {
            guard let pending = pendingJournal else { return }
            pendingJournal = nil
            openJournal = pending
        }) {
            if let book = model.book {
                AllJournalsSheet(book: book, entries: model.journals) { entry in
                    pendingJournal = entry
                    showAllJournals = false
                }
            }
        }
        .sheet(item: $openJournal) { entry in
            JournalDrawer(
                entry: entry,
                isMine: entry.authorId == app.user?.id
            ) {
                openJournal = nil
                composing = .editing(entry)
            } onDelete: {
                openJournal = nil
                Task {
                    await UserDataService.deleteJournal(entry)
                    await model.load(uuid: uuid)
                }
            }
        }
        // Outermost, so the stops, bars, chrome, and every sheet above all
        // inherit the book-toned accent.
        .environment(\.palette, bookPalette)
        .tint(bookPalette.accentColor)
    }

    /// The screen's palette: the theme re-keyed to this book's tone, resolved
    /// the same way the cover plate and art halo resolve it — so a coverless
    /// book's controls match the plate it is showing.
    private var bookPalette: Palette {
        guard let book = model.book else { return palette }
        return palette.accented(by: CoverIdentity(book).tone)
    }

    /// Open the quote-card sheet on a kept line's passage.
    private func quote(_ highlight: Highlight) {
        guard let text = HighlightRow.quotable(highlight) else { return }
        quoteTarget = QuoteRequest(text: text)
    }

    /// Open the player on the file the picker chose, once its sheet is gone.
    private func openPickedAudioFile() {
        guard let file = pickedAudioFile, let book = model.book else { return }
        pickedAudioFile = nil
        Presentation.shared.openPlayer(book, fileID: file.id)
    }

    // MARK: - Shell

    /// Whether this reader opted into the snap-stop marquee. Off — the
    /// default — renders the page as the flow: one snap stop (the cover,
    /// whole), then every section in a single continuous list.
    private var usesScrollStops: Bool {
        app.user?.bookDetailScrollStops ?? false
    }

    private func content(_ book: Book) -> some View {
        GeometryReader { geometry in
            // The scroller and art ignore the safe areas, so the rest
            // geometry has to be derived from the full screen, not the
            // inset-trimmed proxy size.
            let size = CGSize(
                width: geometry.size.width
                    + geometry.safeAreaInsets.leading + geometry.safeAreaInsets.trailing,
                height: geometry.size.height
                    + geometry.safeAreaInsets.top + geometry.safeAreaInsets.bottom
            )
            let restTop = DetailRead.restTop(width: size.width, height: size.height)

            ZStack {
                ScreenBackground()
                DetailArtLayer(book: book, page: page, lift: lift)
                    .ignoresSafeArea()
                if usesScrollStops {
                    snapScroller(book, restTop: restTop)
                        .ignoresSafeArea()
                } else {
                    flowScroller(book, restTop: restTop)
                        .ignoresSafeArea()
                }
            }
            .overlay(alignment: .top) {
                if !usesScrollStops { FlowNavStrip(shown: flowPast) }
            }
            .overlay(alignment: .top) { chromeDiscs(book) }
            .overlay(alignment: .trailing) {
                if usesScrollStops {
                    DetailDotRail(at: at) { stop in
                        withAnimation(Motion.settle) { scrollTarget = stop.rawValue }
                    }
                }
            }
            .overlay(alignment: .bottomTrailing) {
                if usesScrollStops { nextHint }
            }
            .overlay(alignment: .bottom) { restFade }
            .overlay(alignment: .bottom) { actionBar(book) }
        }
    }

    private func snapScroller(_ book: Book, restTop: CGFloat) -> some View {
        ScrollView(.vertical) {
            VStack(spacing: 0) {
                // The rest run: scrolling it away is what lifts the panel,
                // and its top edge is the snap target the panel rests at.
                Color.clear
                    .frame(height: restTop)
                    .id(Self.restMarkerID)
                ForEach(DetailStop.allCases) { stop in
                    stopSection(stop, book)
                        .id(stop.rawValue)
                }
            }
            .scrollTargetLayout()
        }
        // View-aligned rather than paged: the rest run makes the first snap
        // length differ from a page, and `.always` keeps the one-stop-per-
        // swipe contract — the same gesture gate the design prescribes.
        .scrollTargetBehavior(.viewAligned(limitBehavior: .always))
        .scrollPosition(id: $scrollTarget)
        .scrollIndicators(.hidden)
        .onScrollGeometryChange(for: DetailScrollState.self) { geometry in
            DetailScrollState(
                offset: geometry.contentOffset.y + geometry.contentInsets.top,
                viewport: geometry.containerSize.height
            )
        } action: { _, state in
            guard state.viewport > 0 else { return }
            let map = DetailRead.scrollMap(
                offset: state.offset,
                restTop: restTop,
                viewport: state.viewport
            )
            lift = map.lift
            page = min(CGFloat(DetailStop.allCases.count - 1), map.page)
            at = Int(page.rounded())
            let settled = map.lift > 0.6
            if settled != lifted {
                withAnimation(Motion.snap) { lifted = settled }
            }
        }
    }

    private func stopSection(_ stop: DetailStop, _ book: Book) -> some View {
        stopPanel(stop, book)
            .containerRelativeFrame(.vertical)
    }

    private func stopPanel(_ stop: DetailStop, _ book: Book) -> some View {
        let isHome = stop == .home

        return VStack(alignment: .leading, spacing: 0) {
            if isHome {
                LiftHandle(lifted: lifted) { toggleLift() }
            } else {
                StopLabel(stop: stop)
            }
            stopContent(stop, book)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(EdgeInsets(
            // A full stop's label must clear the chrome discs, which end
            // ~105pt below the top edge on current devices. Home earns that
            // clearance as it lifts — at rest its top edge is far below the
            // discs.
            top: isHome ? 14 + 114 * lift : 128,
            leading: 22,
            bottom: Self.barClearance,
            trailing: 30
        ))
        .background {
            // The panel rides over the artwork: blur what shows through, then
            // tint with the page ground. The material already darkens what it
            // blurs, so the tint stays lighter at Home — heavier and the art
            // it rides over reads as a hard cut instead of a ghost. Past Home
            // the art has faded, so the panel goes nearly opaque and simply
            // is the screen. Home moves between those poles as it lifts:
            // resting below the art it owns its strip, lifted it ghosts the
            // artwork behind it.
            ZStack {
                Rectangle().fill(.ultraThinMaterial)
                palette.bg0Color.opacity(isHome ? 0.60 + 0.22 * lift : 0.82)
            }
        }
        .overlay(alignment: .top) {
            if isHome { Hairline().opacity(1 - lift) }
        }
        .clipped()
    }

    /// The handle's tap: jump the scroller to the other Home position.
    private func toggleLift() {
        withAnimation(Motion.settle) {
            scrollTarget = lifted ? Self.restMarkerID : DetailStop.home.rawValue
        }
    }

    // MARK: - Flow (Option B)

    /// The marquee unrolled: the rest run is the one snap stop, and past it
    /// every section runs on in a single continuous list. Only the hero
    /// region snaps (`FlowSnapBehavior`); the list itself scrolls free.
    private func flowScroller(_ book: Book, restTop: CGFloat) -> some View {
        ScrollView(.vertical) {
            VStack(spacing: 0) {
                Color.clear
                    .frame(height: max(0, restTop - DetailRead.flowNavPeek))
                    .id(Self.restMarkerID)
                // The body's snap position: this marker's top edge, a
                // nav strip's worth of art short of the cover's end.
                Color.clear
                    .frame(height: DetailRead.flowNavPeek)
                    .id(Self.flowLiftedMarkerID)
                flowBody(book)
            }
            .scrollTargetLayout()
        }
        .scrollTargetBehavior(FlowSnapBehavior(restTop: restTop))
        .scrollPosition(id: $scrollTarget)
        .scrollIndicators(.hidden)
        .onScrollGeometryChange(for: DetailScrollState.self) { geometry in
            DetailScrollState(
                offset: geometry.contentOffset.y + geometry.contentInsets.top,
                viewport: geometry.containerSize.height
            )
        } action: { _, state in
            guard state.viewport > 0 else { return }
            let map = DetailRead.flowMap(
                offset: state.offset,
                restTop: restTop,
                navPeek: DetailRead.flowNavPeek
            )
            lift = map.lift
            page = map.page
            if map.past != flowPast {
                withAnimation(Motion.snap) { flowPast = map.past }
            }
            let settled = map.lift > 0.6
            if settled != lifted {
                withAnimation(Motion.snap) { lifted = settled }
            }
        }
    }

    /// The list itself: the lift cue, then every section in stop order,
    /// each introduced by its number and a hairline — and nothing capped,
    /// so every kept line and journal entry is in the list.
    private func flowBody(_ book: Book) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            FlowCue(visible: !lifted) {
                withAnimation(Motion.settle) { scrollTarget = Self.flowLiftedMarkerID }
            }
            ForEach(DetailStop.allCases) { stop in
                VStack(alignment: .leading, spacing: 0) {
                    if stop != .home {
                        Hairline()
                            .padding(.top, 34)
                        FlowSectionLabel(stop: stop)
                            .padding(.top, 30)
                    }
                    stopContent(stop, book, uncapped: true)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .padding(EdgeInsets(top: 13, leading: 22, bottom: 132, trailing: 30))
        .background {
            // Slightly lighter than a full stop's 0.82: the art ghosts
            // through near the top and the ground is behind the rest.
            ZStack {
                Rectangle().fill(.ultraThinMaterial)
                palette.bg0Color.opacity(0.74)
            }
        }
        .overlay(alignment: .top) { Hairline() }
    }

    /// At rest the panel's lower content runs on under the action bar; this
    /// fades it into the ground the way the design masks it out, and lifts
    /// away with the panel.
    private var restFade: some View {
        LinearGradient(
            stops: [
                .init(color: palette.bg0Color.opacity(0), location: 0),
                .init(color: palette.bg0Color, location: 0.62),
                .init(color: palette.bg0Color, location: 1),
            ],
            startPoint: .top,
            endPoint: .bottom
        )
        .frame(height: 170)
        .opacity(1 - lift)
        .allowsHitTesting(false)
        .ignoresSafeArea(edges: .bottom)
    }

    @ViewBuilder
    private func stopContent(
        _ stop: DetailStop, _ book: Book, uncapped: Bool = false
    ) -> some View {
        switch stop {
        case .home:
            StopHome(
                book: book,
                model: model,
                lifted: lifted,
                onMore: { showDescription = true },
                onAlignment: { showAlignment = true },
                onRemovedWishlist: { model.wishlistEntry = nil }
            )
        case .stats:
            StopStats(book: book, model: model)
        case .highlights:
            StopHighlights(
                book: book,
                model: model,
                uncapped: uncapped,
                onQuote: { quote($0) },
                onAll: { showAllHighlights = true }
            )
        case .journals:
            StopJournals(
                book: book,
                model: model,
                uncapped: uncapped,
                onWrite: {
                    composing = .new
                },
                onOpen: { openJournal = $0 },
                onAll: { showAllJournals = true }
            )
        case .files:
            StopFiles(
                book: book,
                model: model,
                onRead: { read(book) },
                onListen: { listen(book) },
                onAlignment: { showAlignment = true }
            )
        case .more:
            // The old Shelf and Recommendations stops, concatenated — the
            // shelf block keeps its chips and strips, minus its author strip,
            // which the recommendations' author cluster already carries.
            VStack(alignment: .leading, spacing: 0) {
                StopShelf(book: book, model: model, authorStrip: false) {
                    showShelfPicker = true
                }
                StopRecommendations(book: book, model: model)
                    .padding(.top, 26)
            }
        }
    }

    // MARK: - Chrome

    /// Whether the chrome discs still float over artwork: the marquee's
    /// first stop, or the flow while the cover has not yet scrolled away.
    private var discsOverArt: Bool {
        usesScrollStops ? at == 0 : !flowPast
    }

    /// Back and ⋯, as glass discs over the art — flat controls once the
    /// panel has taken the screen and there is no artwork left to blur.
    private func chromeDiscs(_ book: Book) -> some View {
        HStack {
            Button {
                Haptics.tap()
                dismiss()
            } label: {
                Image(systemName: "chevron.left")
                    .font(.system(size: 16, weight: .semibold))
            }
            .buttonStyle(DiscButtonStyle(overArt: discsOverArt))
            .accessibilityLabel("Back")

            Spacer()

            Menu {
                Button {
                    showShelfPicker = true
                } label: {
                    Label("Add to shelf", systemImage: "square.stack")
                }
                Button {
                    showBookmarks = true
                } label: {
                    Label("Bookmarks", systemImage: "bookmark")
                }
                if app.user?.canEdit == true {
                    NavigationLink(value: Destination.metadataEdit(uuid: book.uuid)) {
                        Label("Edit metadata", systemImage: "pencil")
                    }
                }
                if app.user?.canDownload == true {
                    Button {
                        Task { await shareBook(book) }
                    } label: {
                        Label("Export file", systemImage: "square.and.arrow.up")
                    }
                }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 15, weight: .semibold))
            }
            .buttonStyle(DiscButtonStyle(overArt: discsOverArt))
            .accessibilityLabel("More")
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .animation(Motion.snap, value: discsOverArt)
    }

    /// A quiet pointer at what the next swipe reaches. Home speaks for
    /// itself and the last stop has nowhere to go.
    @ViewBuilder
    private var nextHint: some View {
        if at > 0, at < DetailStop.allCases.count - 1,
           let next = DetailStop(rawValue: at + 1)
        {
            Text("swipe up — next: \(next.name)".uppercased())
                .font(.monoUI(8.5))
                .tracking(1.4)
                .foregroundStyle(palette.ink3Color)
                .padding(.trailing, 34)
                .padding(.bottom, Self.barClearance - 24)
                .allowsHitTesting(false)
                .transition(.opacity)
        }
    }

    // MARK: - Action bar

    /// The persistent CTA row: Resume/Read (+ Listen for a dual-format book),
    /// or Find a copy · check in for a wishlisted book with no files.
    private func actionBar(_ book: Book) -> some View {
        HStack(spacing: 9) {
            if model.isWishlistOnly {
                Button {
                    Haptics.tap()
                    StoreLink.open(book: book, with: openURL)
                } label: {
                    Text("Find a copy")
                }
                .buttonStyle(BarCTAStyle())

                Button {
                    Haptics.tap()
                    showCheckIn = true
                } label: {
                    Image(systemName: "checkmark")
                        .font(.system(size: 17, weight: .semibold))
                }
                .buttonStyle(BarGlassStyle())
                .accessibilityLabel("Check in a copy")
            } else if !book.hasEbook, !book.hasAudiobook {
                // A physical-only record (a check-in): there is nothing to
                // open, so the bar states that instead of routing into a
                // reader or player that would come up empty.
                Text(book.hasPhysical ? "On your shelf — physical copy" : "No readable copy")
                    .font(.ui(14, weight: .medium))
                    .foregroundStyle(palette.ink2Color)
                    .frame(maxWidth: .infinity)
                    .frame(height: 50)
                    .background(
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .fill(palette.bg2Color.opacity(0.6))
                    )
            } else {
                // Label and destination stay in lockstep: a CTA speaking an
                // audio position opens the player, not the reader at page one.
                let toPlayer = DetailRead.resumesIntoPlayer(
                    hasEbook: book.hasEbook,
                    hasAudiobook: book.hasAudiobook,
                    epubStarted: model.epubProgress != nil,
                    audioSeconds: model.audioProgress?.audioPositionSeconds
                )

                Button {
                    Haptics.tap()
                    if toPlayer { listen(book) } else { read(book) }
                } label: {
                    Text(DetailRead.resumeLabel(
                        hasEbook: book.hasEbook,
                        hasAudiobook: book.hasAudiobook,
                        epubStarted: model.epubProgress != nil,
                        epubPercent: model.epubProgress?.progressPercent,
                        audioSeconds: model.audioProgress?.audioPositionSeconds
                    ))
                }
                .buttonStyle(BarCTAStyle())

                if book.hasEbook, book.hasAudiobook {
                    // The other format keeps a door: Listen beside a reading
                    // CTA, Read beside a listening one.
                    Button {
                        Haptics.tap()
                        if toPlayer { read(book) } else { listen(book) }
                    } label: {
                        Image(systemName: toPlayer ? "book" : "headphones")
                            .font(.system(size: 17, weight: .semibold))
                    }
                    .buttonStyle(BarGlassStyle())
                    .accessibilityLabel(toPlayer ? "Read" : "Listen")

                    Button {
                        Haptics.tap()
                        immersiveRead(book)
                    } label: {
                        ImmersiveReadMark()
                    }
                    .buttonStyle(BarGlassStyle())
                    .accessibilityLabel("Immersive read")
                    .accessibilityIdentifier("bar-immersive-read")
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 11)
        .padding(.bottom, 10)
        .background {
            ZStack {
                Rectangle().fill(.ultraThinMaterial)
                palette.bg0Color.opacity(0.6)
            }
            .overlay(alignment: .top) { Hairline() }
            .ignoresSafeArea(edges: .bottom)
        }
    }

    private func read(_ book: Book) {
        Presentation.shared.openReader(book)
    }

    /// The immersive read: the reader with the audiobook running under it —
    /// the docked mini bar keeps playback controllable while reading. Audio
    /// loads in the background (it resolves the saved-position file itself,
    /// and re-opening the playing book is a no-op) while the reader opens.
    private func immersiveRead(_ book: Book) {
        Task { await player.load(book: book) }
        Presentation.shared.openReader(book)
    }

    /// A book with one audiobook file opens straight into the player; one
    /// with several asks which — they don't share a timeline, so "Listen"
    /// alone can't say where to put the needle. The player resolves the
    /// saved-position file on its own for the single-file case.
    private func listen(_ book: Book) {
        if book.audioFiles.count > 1 {
            showAudioFilePicker = true
        } else {
            Presentation.shared.openPlayer(book)
        }
    }

    private func shareBook(_ book: Book) async {
        // Reuse the offline copy when one exists so an export costs nothing.
        var url = downloads.localURL(for: book.uuid, kind: .ebook)
        if url == nil {
            guard let data = try? await APIClient.shared.data(for: "/api/ebooks/\(book.uuid)/download")
            else { return }
            let temp = FileManager.default.temporaryDirectory
                .appendingPathComponent("\(book.displayTitle).epub")
            try? data.write(to: temp)
            url = temp
        }
        guard let url else { return }
        ShareSheet.present(items: [url])
    }
}

/// Snapshot of the scroll geometry the shell reacts to.
private struct DetailScrollState: Equatable {
    var offset: CGFloat
    var viewport: CGFloat
}

/// The desktop dot rail, moved to the panel's right edge. A view of its own
/// so it reads the palette from the environment — the screen re-keys that to
/// the book's tone, and the active dot must carry the book accent, not the
/// app's.
struct DetailDotRail: View {
    let at: Int
    var onJump: (DetailStop) -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            ForEach(DetailStop.allCases) { stop in
                Button {
                    Haptics.select()
                    onJump(stop)
                } label: {
                    Circle()
                        .fill(at == stop.rawValue ? palette.accentColor : palette.bg3Color)
                        .frame(width: 7, height: 7)
                        .scaleEffect(at == stop.rawValue ? 1.35 : 1)
                        // The dot is the indicator, not the target — the row
                        // is sized for a finger, near the 44pt guideline.
                        .frame(width: 44, height: 26)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(stop.name)
            }
        }
        .padding(.vertical, 5)
        // The visible pill stays slim while the touch strip stays wide — the
        // capsule is drawn narrower than the rail it backs.
        .background {
            Capsule().fill(palette.bg0Color.opacity(0.45))
                .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
                .frame(width: 19)
        }
        .offset(y: 90)
        .animation(Motion.snap, value: at)
    }
}

// MARK: - Art layer

/// The artwork, full-bleed off the top edge — the cover when the record has
/// one, the generated plate at the same size otherwise, so every book takes
/// the identical path. While the Home panel rests the art shows whole —
/// unmasked, nothing cropped; as the panel lifts, it closes down to the
/// window it pans inside across the stops, and dissolves to a wash as the
/// panel takes the screen.
struct DetailArtLayer: View {
    let book: Book
    let page: CGFloat
    /// The Home panel's rest→lifted progress — drives the whole→windowed
    /// transition.
    var lift: CGFloat = 1

    private var fade: Double {
        min(1, Double(page) * 1.25)
    }

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            let imageHeight = width * 1.5
            // The window opens to the whole image at rest and closes to the
            // pan height as the panel lifts; the mask's fade-out arrives
            // with it, so the resting artwork keeps its bottom edge.
            let windowHeight = BookDetailView.artHeight
                + (imageHeight - BookDetailView.artHeight) * (1 - lift)
            let travel = max(0, imageHeight - windowHeight)
            let progress = min(1, page / CGFloat(DetailStop.allCases.count - 1))
            let fadeStart = 0.58 + 0.42 * (1 - lift)

            art
                .frame(width: width, height: imageHeight)
                .offset(y: -progress * travel)
                .frame(width: width, height: windowHeight, alignment: .top)
                .clipped()
                // Dark scrim off the top edge so the status bar and discs
                // stay legible over bright artwork.
                .overlay(alignment: .top) {
                    LinearGradient(
                        colors: [.black.opacity(0.5), .clear],
                        startPoint: .top,
                        endPoint: .bottom
                    )
                    .frame(height: BookDetailView.artHeight * 0.2)
                }
                .mask {
                    LinearGradient(
                        stops: [
                            .init(color: .black, location: 0),
                            .init(color: .black, location: fadeStart),
                            .init(color: .clear, location: 1),
                        ],
                        startPoint: .top,
                        endPoint: .bottom
                    )
                }
        }
        .opacity(1 - fade * 0.85)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    /// The image itself — same frame either way; a coverless record just
    /// skips the fetch that could only 404.
    @ViewBuilder
    private var art: some View {
        if book.coverURL != nil {
            RemoteImage(
                path: "/api/covers/\(book.uuid)",
                alternates: ["/api/thumbs/\(book.uuid)/lg"]
            ) {
                Color.clear
            }
            // Permanent backdrop, not a loading placeholder — the same rule
            // as `BookCover`: a transparent or degenerate cover image loads
            // "successfully" and would otherwise leave a black band.
            .background { GeneratedCoverPlate(identity: CoverIdentity(book)) }
        } else {
            GeneratedCoverPlate(identity: CoverIdentity(book))
        }
    }
}

// MARK: - Chrome styles

/// The 38pt circular chrome control. Glass while artwork shows through it;
/// a plain filled disc once the panel has taken the screen.
struct DiscButtonStyle: ButtonStyle {
    var overArt: Bool

    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(overArt ? Color.white : palette.ink1Color)
            .frame(width: 38, height: 38)
            .background {
                if overArt {
                    Circle().fill(.ultraThinMaterial)
                        .environment(\.colorScheme, .dark)
                } else {
                    Circle().fill(palette.bg2Color)
                }
            }
            .overlay {
                Circle().strokeBorder(
                    overArt ? .white.opacity(0.18) : palette.line2.color,
                    lineWidth: 0.5
                )
            }
            .contentShape(Circle())
            .scaleEffect(configuration.isPressed ? 0.94 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

/// The action bar's primary CTA: full-width, accent-filled.
struct BarCTAStyle: ButtonStyle {
    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(15, weight: .semibold))
            .foregroundStyle(palette.accentInk.color)
            .lineLimit(1)
            .frame(maxWidth: .infinity)
            .frame(height: 50)
            .background(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(palette.accentColor)
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

/// The immersive-read glyph: a page beside sound bars, drawn to match the
/// design's mark — no SF Symbol says "read along".
struct ImmersiveReadMark: View {
    var size: CGFloat = 17

    var body: some View {
        // Laid out on the mark's 24pt grid, scaled to `size`.
        let unit = size / 24
        let stroke = 1.9 * unit

        HStack(alignment: .center, spacing: 3.5 * unit) {
            RoundedRectangle(cornerRadius: 2 * unit, style: .continuous)
                .strokeBorder(lineWidth: stroke)
                .frame(width: 8 * unit, height: 14 * unit)
            HStack(alignment: .center, spacing: 1.1 * unit) {
                bar(8, unit: unit, stroke: stroke)
                bar(12, unit: unit, stroke: stroke)
                bar(5, unit: unit, stroke: stroke)
            }
        }
        .frame(width: size, height: size)
    }

    private func bar(_ height: CGFloat, unit: CGFloat, stroke: CGFloat) -> some View {
        Capsule()
            .frame(width: stroke, height: height * unit)
    }
}

/// The action bar's square secondary: glass, icon-only.
struct BarGlassStyle: ButtonStyle {
    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(palette.ink0Color)
            .frame(width: 50, height: 50)
            .background(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(palette.bg2Color.opacity(0.8))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 0.5)
            )
            .scaleEffect(configuration.isPressed ? 0.94 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}
