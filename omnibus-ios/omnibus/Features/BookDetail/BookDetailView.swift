//  BookDetailView.swift
//  The book detail marquee: full-bleed cover art running off the top edge,
//  a translucent panel riding over its lower edge, and the page organised as
//  seven snap stops on a vertical pager — Home · Shelf · Stats · Highlights ·
//  Journals · The files · Recommendations. Chrome is immersive: glass discs
//  over the art, a tappable dot rail, and a persistent bottom action bar so
//  Resume never scrolls away.

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
                    self.highlights = items
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

/// The seven stops, in scroll order.
enum DetailStop: Int, CaseIterable, Identifiable {
    case home, shelf, stats, highlights, journals, files, recommendations

    var id: Int { rawValue }

    var name: String {
        switch self {
        case .home: "Home"
        case .shelf: "Shelf"
        case .stats: "Stats"
        case .highlights: "Highlights"
        case .journals: "Journals"
        case .files: "The files"
        case .recommendations: "Recommendations"
        }
    }
}

struct BookDetailView: View {
    let uuid: String

    init(uuid: String) {
        self.uuid = uuid
    }

    /// Where the panel's top edge sits at the Home stop — how much cover art
    /// stays in view before the page becomes the panel.
    static let artTop: CGFloat = 300
    /// Window the artwork pans inside.
    static let artHeight: CGFloat = 470
    /// Clearance the panel keeps for the action bar.
    static let barClearance: CGFloat = 108

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @Environment(AppState.self) private var app
    @Environment(AudioPlayer.self) private var player
    @Environment(\.bookZoomNamespace) private var bookZoom
    @State private var model = BookDetailModel()
    @State private var showJournalComposer = false
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
    /// The file chosen in the picker, opened from the sheet's `onDismiss` —
    /// presenting the full-screen player while the sheet is still up would
    /// be refused.
    @State private var pickedAudioFile: BookFileInfo?
    /// Non-nil when the composer is open on an existing entry.
    @State private var editingJournal: JournalEntry?
    /// Continuous page position, 0...6 — drives the art pan and fade.
    @State private var page: CGFloat = 0
    /// The stop the scroller has settled nearest to.
    @State private var at: Int = 0
    /// Scroll-position binding for the dot rail's jumps. Written on a tap;
    /// the scroller keeps it updated as pages snap.
    @State private var scrollTarget: Int?

    private var downloads = DownloadManager.shared

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
        .bookZoomDestination(uuid, in: bookZoom)
        .task { await model.load(uuid: uuid) }
        .refreshTask { await model.load(uuid: uuid) }
        // The tab bar and mini player yield the bottom edge to the action
        // bar while this screen is up. Counted, not toggled: a series-strip
        // tap pushes a second detail over this one.
        .onAppear { Presentation.shared.pushImmersiveDetail() }
        .onDisappear { Presentation.shared.popImmersiveDetail() }
        .sheet(isPresented: $showAlignment) {
            if let book = model.book {
                AlignmentSheet(book: book) {
                    Task { await model.load(uuid: uuid) }
                }
            }
        }
        .sheet(isPresented: $showJournalComposer) {
            if let book = model.book {
                JournalComposer(book: book, editing: editingJournal) {
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
        .sheet(isPresented: $showAllHighlights) {
            if let book = model.book {
                AllHighlightsSheet(book: book, highlights: model.highlights)
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
                editingJournal = entry
                showJournalComposer = true
            } onDelete: {
                openJournal = nil
                Task {
                    await UserDataService.deleteJournal(entry)
                    await model.load(uuid: uuid)
                }
            }
        }
    }

    /// Open the player on the file the picker chose, once its sheet is gone.
    private func openPickedAudioFile() {
        guard let file = pickedAudioFile, let book = model.book else { return }
        pickedAudioFile = nil
        Presentation.shared.openPlayer(book, fileID: file.id)
    }

    // MARK: - Shell

    private func content(_ book: Book) -> some View {
        ZStack {
            ScreenBackground()
            DetailArtLayer(book: book, page: page)
                .ignoresSafeArea()
            snapScroller(book)
                .ignoresSafeArea()
        }
        .overlay(alignment: .top) { chromeDiscs(book) }
        .overlay(alignment: .trailing) { dotRail }
        .overlay(alignment: .bottomTrailing) { nextHint }
        .overlay(alignment: .bottom) { actionBar(book) }
    }

    private func snapScroller(_ book: Book) -> some View {
        ScrollView(.vertical) {
            VStack(spacing: 0) {
                ForEach(DetailStop.allCases) { stop in
                    stopSection(stop, book)
                        .id(stop.rawValue)
                }
            }
            .scrollTargetLayout()
        }
        .scrollTargetBehavior(.paging)
        .scrollPosition(id: $scrollTarget)
        .scrollIndicators(.hidden)
        .onScrollGeometryChange(for: DetailScrollState.self) { geometry in
            DetailScrollState(
                offset: geometry.contentOffset.y + geometry.contentInsets.top,
                viewport: geometry.containerSize.height
            )
        } action: { _, state in
            guard state.viewport > 0 else { return }
            let raw = state.offset / state.viewport
            page = min(CGFloat(DetailStop.allCases.count - 1), max(0, raw))
            at = Int(page.rounded())
        }
    }

    private func stopSection(_ stop: DetailStop, _ book: Book) -> some View {
        VStack(spacing: 0) {
            if stop == .home {
                Color.clear.frame(height: Self.artTop)
            }
            stopPanel(stop, book)
        }
        .containerRelativeFrame(.vertical)
    }

    private func stopPanel(_ stop: DetailStop, _ book: Book) -> some View {
        let isHome = stop == .home

        return VStack(alignment: .leading, spacing: 0) {
            StopLabel(stop: stop)
            stopContent(stop, book)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(EdgeInsets(
            top: isHome ? 14 : 104,
            leading: 22,
            bottom: Self.barClearance,
            trailing: 30
        ))
        .background {
            // The panel rides over the artwork: blur what shows through, then
            // tint with the page ground. The material already darkens what it
            // blurs, so the tint stays light at Home — heavier and the art it
            // rides over reads as a hard cut instead of a ghost. Past Home the
            // art has faded, so the panel goes nearly opaque and simply is
            // the screen.
            ZStack {
                Rectangle().fill(.ultraThinMaterial)
                palette.bg0Color.opacity(isHome ? 0.30 : 0.82)
            }
        }
        .overlay(alignment: .top) {
            if isHome { Hairline() }
        }
        .clipped()
    }

    @ViewBuilder
    private func stopContent(_ stop: DetailStop, _ book: Book) -> some View {
        switch stop {
        case .home:
            StopHome(
                book: book,
                model: model,
                onMore: { showDescription = true },
                onRemovedWishlist: { model.wishlistEntry = nil }
            )
        case .shelf:
            StopShelf(book: book, model: model) { showShelfPicker = true }
        case .stats:
            StopStats(book: book, model: model)
        case .highlights:
            StopHighlights(book: book, model: model) { showAllHighlights = true }
        case .journals:
            StopJournals(
                book: book,
                model: model,
                onWrite: {
                    editingJournal = nil
                    showJournalComposer = true
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
        case .recommendations:
            StopRecommendations(book: book, model: model)
        }
    }

    // MARK: - Chrome

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
            .buttonStyle(DiscButtonStyle(overArt: at == 0))
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
            .buttonStyle(DiscButtonStyle(overArt: at == 0))
            .accessibilityLabel("More")
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .animation(Motion.snap, value: at == 0)
    }

    /// The desktop dot rail, moved to the panel's right edge.
    private var dotRail: some View {
        VStack(spacing: 0) {
            ForEach(DetailStop.allCases) { stop in
                Button {
                    Haptics.select()
                    withAnimation(Motion.settle) { scrollTarget = stop.rawValue }
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

// MARK: - Art layer

/// The cover, full-bleed off the top edge. Photographic art is taller than
/// its window and pans as the page moves through the stops; a generated
/// plate is shown whole instead — cropping one would eat its own layout.
/// Both dissolve to a wash as the panel takes the screen.
struct DetailArtLayer: View {
    let book: Book
    let page: CGFloat

    @Environment(\.palette) private var palette

    /// How much taller the panned image is than its window, at 2:3 on a
    /// 402pt-wide device — the travel budget the pan spends across the stops.
    private var fade: Double {
        min(1, Double(page) * 1.25)
    }

    var body: some View {
        Group {
            if book.coverURL != nil {
                pannedArt
            } else {
                plate
            }
        }
        .opacity(1 - fade * 0.85)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private var pannedArt: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            let imageHeight = width * 1.5
            let travel = max(0, imageHeight - BookDetailView.artHeight)
            let progress = min(1, page / CGFloat(DetailStop.allCases.count - 1))

            RemoteImage(
                path: "/api/covers/\(book.uuid)",
                alternates: ["/api/thumbs/\(book.uuid)/lg"]
            ) {
                GeneratedCoverPlate(identity: CoverIdentity(book))
            }
            .frame(width: width, height: imageHeight)
            .offset(y: -progress * travel)
            .frame(width: width, height: BookDetailView.artHeight, alignment: .top)
            .clipped()
            // Dark scrim off the top edge so the status bar and discs stay
            // legible over bright artwork.
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
                        .init(color: .black, location: 0.58),
                        .init(color: .clear, location: 1),
                    ],
                    startPoint: .top,
                    endPoint: .bottom
                )
            }
        }
        .frame(height: BookDetailView.artHeight)
    }

    /// The typographic plate, whole, in a soft accent halo.
    private var plate: some View {
        let tone = CoverIdentity(book).tone

        return ZStack(alignment: .top) {
            RadialGradient(
                colors: [
                    OKLCH(0.45, tone.c * 0.9, tone.h).color.opacity(0.55),
                    .clear,
                ],
                center: UnitPoint(x: 0.5, y: 0.3),
                startRadius: 0,
                endRadius: 280
            )
            .frame(height: BookDetailView.artHeight)

            BookCover(identity: CoverIdentity(book), size: .lg, cornerRadius: 7)
                .frame(width: 190)
                .coverShadow(1.2)
                .padding(.top, 58)
        }
        .frame(maxWidth: .infinity)
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
