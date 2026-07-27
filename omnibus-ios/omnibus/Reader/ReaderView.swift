//  ReaderView.swift
//  Native chrome around the epub.js stage: tap zones for page turns, an
//  auto-hiding top/bottom bar, the sheets, and the passage menu that turns a
//  selection into a highlight, a note, or a quote card.

import SwiftUI
import Translation

/// A position worth being able to get back to after a scrub.
struct ReturnPoint: Equatable {
    let cfi: String
    let page: Int
}

/// A passage on its way to the quote-card sheet. Identity is per request, so
/// making a card of the same passage twice reopens the sheet.
struct QuoteRequest: Identifiable {
    let id = UUID()
    let text: String
}

struct ReaderView: View {
    let book: Book

    init(book: Book) {
        self.book = book
    }

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) private var appState

    @State private var controller = ReaderController()
    @State private var chromeVisible = true
    @State private var showSettings = false
    @State private var showContents = false
    @State private var contentsTab: ReaderContentsSheet.Tab = .contents
    @State private var showMenu = false
    @State private var highlights: [Highlight] = []
    /// Only the count is needed here — the list itself is the sheet's business.
    @State private var bookmarkCount = 0
    /// Where a scrub started, so there's a way back from a mis-drag.
    @State private var returnPoint: ReturnPoint?
    /// Live position while the Contents row is being scrubbed, driving the
    /// chapter/page readout above it.
    @State private var scrubFraction: Double?
    /// Flips the ribbon solid for a beat after saving a bookmark — the only
    /// confirmation that an instant, invisible action actually happened.
    @State private var justBookmarked = false
    /// The highlight whose note is being written, driving the composer sheet.
    @State private var noteTarget: Highlight?
    /// The passage being turned into a card, driving the quote sheet.
    @State private var quoteTarget: QuoteRequest?
    /// Translation is a system presentation over the reader, not a sheet of
    /// ours, so it needs its own live text rather than an optional target.
    @State private var translateText = ""
    @State private var showTranslate = false
    /// True while a selection handle is under a finger. The menu comes down
    /// for the duration — one that chases a drag can't be read, and Books
    /// likewise brings it back only on release.
    @State private var adjustingSelection = false
    @State private var startCFI: String?
    /// The record the reader actually opened on, so a later answer from the
    /// server can be judged newer or older than what this device knew.
    @State private var openedProgress: ProgressRecord?
    /// A further position another device reached, waiting on the reader to
    /// accept it. Never applied on its own.
    @State private var syncOffer: String?
    @State private var didConfigure = false
    /// When the current stretch of reading began, or `nil` while the app is in
    /// the background. Cleared and restarted across a backgrounding rather than
    /// run from open to close, because what goes to the server is *reading*
    /// time: a wall-clock span counted a book left open overnight as eight
    /// hours read.
    @State private var sessionStart: Date?
    @State private var lastSave = Date.distantPast

    /// How long a single stretch of reading may run before it is reported as
    /// its own session rather than held until the book closes.
    private static let sessionCheckpointInterval: TimeInterval = 300

    private var bridge = ReaderBridge.shared

    var body: some View {
        ZStack {
            readerPage.ignoresSafeArea()

            if didConfigure {
                ReaderWebView(controller: controller, bookUUID: book.uuid)
                    .ignoresSafeArea()
                    .opacity(controller.isReady ? 1 : 0)
                    // Fade the first paint in rather than letting a
                    // half-laid-out page flash — the reader is the one surface
                    // where a transient wrong state is most visible.
                    .animation(Motion.page, value: controller.isReady)
            }

            if !controller.isReady && !controller.failed {
                LoadingView(label: "Opening \(book.displayTitle)")
            }

            if controller.failed {
                ErrorStateView(
                    message: controller.failureMessage ?? "This book couldn't be opened."
                ) { dismiss() }
            }

            persistentIndicators

            if let syncOffer {
                SyncOfferBanner(
                    onGo: {
                        controller.display(syncOffer)
                        startCFI = syncOffer
                        dismissSyncOffer()
                    },
                    onDismiss: dismissSyncOffer
                )
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            // Sits under the chrome so the menu's own rows stay tappable, and
            // over the page so a tap outside closes the menu instead of
            // turning a page behind it.
            if showMenu {
                Color.clear
                    .contentShape(Rectangle())
                    .ignoresSafeArea()
                    .onTapGesture { closeMenu() }
            }

            chrome

            selectionLayer

            passageMenu
        }
        .animation(Motion.snap, value: passage?.id)
        .statusBarHidden(!chromeVisible)
        .persistentSystemOverlays(chromeVisible ? .automatic : .hidden)
        // The status bar sits on the page, so it has to read against the
        // reading theme — white-on-white otherwise. Sheets opt back out below.
        .preferredColorScheme(pageScheme)
        .task { await prepare() }
        // The glue owns tap zones and swipe-to-turn inside the page; a centre
        // tap arrives here as a token rather than us layering a gesture view
        // over the web view and starving it of touches.
        .onChange(of: controller.chromeToggleToken) { _, _ in
            withAnimation(Motion.settle) {
                chromeVisible.toggle()
                // Hiding the chrome takes the menu with it — it hangs off the
                // button that just left.
                if !chromeVisible { showMenu = false }
            }
        }
        .onChange(of: controller.location?.cfi) { _, _ in
            Task { await persist(force: false) }
        }
        // A handle drag ends when the finger lifts — unless the selection goes
        // out from under it first, which a re-pagination does (the glue drops
        // the selection on `relocated`, and a rotation or the audio dock
        // appearing re-paginates). The handles leave the hierarchy mid-gesture,
        // SwiftUI never delivers `onEnded`, and the flag would pin the passage
        // menu shut for the rest of the session.
        .onChange(of: controller.selection == nil) { _, gone in
            if gone { adjustingSelection = false }
        }
        .onChange(of: bridge.pendingCFI) { _, cfi in
            guard let cfi else { return }
            controller.display(cfi)
            bridge.pendingCFI = nil
        }
        .onDisappear {
            Task { await finish() }
        }
        // The sheets are app surfaces, not page surfaces, so they stay on the
        // app's appearance even when the page is set to Light or Sepia.
        .sheet(isPresented: $showSettings) {
            ReaderSettingsSheet(controller: controller)
                .preferredColorScheme(appScheme)
        }
        .sheet(isPresented: $showContents) {
            ReaderContentsSheet(
                book: book,
                controller: controller,
                highlights: highlights,
                initialTab: contentsTab,
                onRemoveHighlight: { highlight in
                    Task { await removeHighlight(highlight) }
                },
                onBookmarkCountChanged: { bookmarkCount = $0 }
            )
            .preferredColorScheme(appScheme)
        }
        .sheet(item: $noteTarget) { highlight in
            NoteComposer(quote: highlight.text, existing: highlight.note) { note in
                Task { await saveNote(note, on: highlight) }
            }
            .preferredColorScheme(appScheme)
        }
        .sheet(item: $quoteTarget) { request in
            QuoteCardSheet(quote: request.text, book: book)
                .preferredColorScheme(appScheme)
        }
        .translationPresentation(isPresented: $showTranslate, text: translateText)
    }

    // MARK: - Passage menu

    /// A passage worth a menu: either a live selection, or a highlight already
    /// on the page that was tapped. One menu serves both — the verbs are the
    /// same, and only whether there is already a colour differs.
    private enum Passage: Equatable {
        case selection(SelectionData)
        case highlight(Highlight, [PageRect])

        var rects: [PageRect] {
            switch self {
            case .selection(let selection): selection.rects
            case .highlight(_, let rects): rects
            }
        }

        /// Identity for the menu, so moving to another passage fades in at the
        /// new one rather than sliding the menu across the page.
        var id: String {
            switch self {
            case .selection(let selection): "sel:\(selection.cfiRange ?? selection.text)"
            case .highlight(let highlight, _): "hl:\(highlight.epubCFIRange)"
            }
        }
    }

    private var passage: Passage? {
        if let tap = controller.tappedAnnotation, let highlight = tappedHighlight(tap) {
            return .highlight(highlight, tap.rects)
        }
        guard let selection = controller.selection,
              !selection.dragging,
              !adjustingSelection,
              !selection.rects.isEmpty
        else { return nil }
        return .selection(selection)
    }

    /// The stored highlight a passage already carries, if any.
    private func storedHighlight(_ passage: Passage) -> Highlight? {
        switch passage {
        case .highlight(let highlight, _):
            return highlight
        case .selection(let selection):
            guard let existing = selection.existing else { return nil }
            return highlights.first { $0.epubCFIRange == existing }
        }
    }

    private func passageText(_ passage: Passage) -> String {
        switch passage {
        case .selection(let selection): selection.text
        case .highlight(let highlight, _): highlight.text ?? ""
        }
    }

    /// The tint and the grabbers. Drawn by the app rather than by WebKit —
    /// see `ReaderSelectionLayer` for why.
    @ViewBuilder
    private var selectionLayer: some View {
        if let selection = controller.selection, !selection.rects.isEmpty {
            ReaderSelectionLayer(
                selection: selection,
                theme: controller.settings.theme,
                onEdgeDragBegan: { edge in
                    adjustingSelection = true
                    controller.beginEdgeDrag(edge)
                },
                onEdgeDragChanged: { point in controller.dragEdge(to: point) },
                onEdgeDragEnded: {
                    adjustingSelection = false
                    controller.endEdgeDrag()
                }
            )
            .transition(.opacity)
        }
    }

    @ViewBuilder
    private var passageMenu: some View {
        if let passage {
            // A tapped highlight has no glue-side state to dismiss it, so the
            // scrim is what closes it — including over the gutters, which
            // would otherwise turn the page out from under a menu about a
            // passage on it. A live selection needs no scrim: the glue clears
            // it on the next tap, and a scrim would swallow handle drags.
            if case .highlight = passage {
                Color.clear
                    .contentShape(Rectangle())
                    .ignoresSafeArea()
                    .onTapGesture { dismissPassage() }
            }

            PassageAnchor(
                rects: passage.rects,
                width: AnnotationMenu.width,
                height: AnnotationMenu.height
            ) { tail in
                let stored = storedHighlight(passage)
                AnnotationMenu(
                    current: stored?.color,
                    hasNote: stored?.note?.nilIfBlank != nil,
                    theme: controller.settings.theme,
                    onColor: { color in Task { await apply(color, to: passage) } },
                    onAction: { action in Task { await run(action, on: passage) } },
                    canRemove: stored != nil,
                    tail: tail
                )
            }
            .id(passage.id)
            .transition(.opacity.combined(with: .scale(scale: 0.94)))
        }
    }

    /// The stored highlight a tapped mark belongs to.
    private func tappedHighlight(_ tap: AnnotationTapData) -> Highlight? {
        highlights.first { $0.epubCFIRange == tap.cfiRange }
    }

    /// Put the menu away, whichever kind of passage raised it.
    private func dismissPassage() {
        withAnimation(Motion.snap) {
            controller.tappedAnnotation = nil
        }
        if controller.selection != nil { controller.clearSelection() }
    }

    private func apply(_ color: HighlightColor, to passage: Passage) async {
        if let stored = storedHighlight(passage) {
            await recolor(stored, to: color)
            if controller.selection != nil { controller.clearSelection() }
            return
        }
        guard case .selection(let selection) = passage else { return }
        await createHighlight(selection, color: color)
    }

    private func run(_ action: PassageAction, on passage: Passage) async {
        let text = passageText(passage)

        switch action {
        case .note:
            if let stored = storedHighlight(passage) {
                dismissPassage()
                noteTarget = stored
            } else if case .selection(let selection) = passage {
                await createHighlightThenNote(selection)
            }

        case .quote:
            dismissPassage()
            quoteTarget = QuoteRequest(text: text)

        case .copy:
            UIPasteboard.general.string = text
            Haptics.success()
            dismissPassage()

        case .lookUp:
            dismissPassage()
            DictionaryLookup.present(text)

        case .translate:
            dismissPassage()
            translateText = text
            showTranslate = true

        case .share:
            dismissPassage()
            ShareSheet.present(items: [text])

        case .remove:
            guard let stored = storedHighlight(passage) else { return }
            if controller.selection != nil { controller.clearSelection() }
            await removeHighlight(stored)
        }
    }

    /// Declining is as final as accepting — re-offering the same position on
    /// the next relocate would nag through the whole chapter.
    private func dismissSyncOffer() {
        withAnimation(Motion.settle) { syncOffer = nil }
    }

    private var readerPage: Color {
        ReaderTheme.pageColor(controller.settings.theme)
    }

    /// The two labels, which stay put whether the buttons are showing or not.
    ///
    /// They sit in the bands `#stage` reserves via `env(safe-area-inset-*)`, so
    /// they never overlap prose, and they take no touches so the page keeps
    /// every gesture. Only the buttons come and go on a centre tap — the
    /// position is quiet enough to live there permanently.
    private var persistentIndicators: some View {
        VStack(spacing: 0) {
            indicator(chapterLabel)
                .padding(.top, Spacing.xs)

            Spacer(minLength: 0)

            indicator(positionLabel)
                .padding(.bottom, Spacing.xs)
        }
        .allowsHitTesting(false)
    }

    /// Given the button's own box rather than its own text height, so a label
    /// sits centred on the line its buttons are on however the type resolves.
    @ViewBuilder
    private func indicator(_ label: String?) -> some View {
        if let label {
            Text(label)
                .font(.ui(12.5))
                .foregroundStyle(barInk.opacity(0.5))
                .lineLimit(1)
                .frame(height: ReaderMenu.buttonSize)
                .padding(.horizontal, 72)
        }
    }

    /// How much further to a natural place to stop — the question you have with
    /// a book open, which a raw page number can't answer. Falls back to the
    /// chapter's own position until epub.js's whole-book locations pass lands.
    private var chapterLabel: String? {
        guard let location = controller.location else { return nil }
        let left = location.chapterPagesLeft
        if left > 0 {
            return left == 1
                ? "1 page left in chapter"
                : "\(left) pages left in chapter"
        }
        // `chapter` is 0 when the current href matched no TOC entry (common in
        // front matter) — omit rather than showing a meaningless "Ch. 0".
        guard location.chapter > 0, location.totalChapters > 0 else { return nil }
        return "Chapter \(location.chapter) of \(location.totalChapters)"
    }

    /// Page numbers need epub.js's whole-book locations pass, which lands
    /// seconds after the first paint; percent stands in until then rather than
    /// a placeholder that jumps.
    private var positionLabel: String? {
        guard let location = controller.location else { return nil }
        if location.hasPageNumbers {
            return "\(location.page) of \(location.totalPages)"
        }
        return location.pct > 0 ? "\(location.pct)%" : nil
    }

    @ViewBuilder
    private var chrome: some View {
        if chromeVisible {
            VStack {
                topBar
                Spacer()
                bottomBar
            }
            .transition(.opacity)
            // Glass resolves against the environment's scheme, not the pixels
            // behind it, so app-dark chrome lands as a grey slab on a white
            // page. Same reason `barInk` follows the reading theme.
            .environment(\.colorScheme, pageScheme)
        }
    }

    /// The scheme the *page* is in, which is what the chrome sits on.
    private var pageScheme: ColorScheme {
        ReaderTheme.isLightPage(controller.settings.theme) ? .light : .dark
    }

    /// The scheme the rest of the app is in.
    private var appScheme: ColorScheme { appState.theme.colorScheme }

    private var topBar: some View {
        HStack {
            if let returnPoint {
                ReaderReturnButton(page: returnPoint.page, ink: barInk) {
                    controller.display(returnPoint.cfi)
                    withAnimation(Motion.snap) { self.returnPoint = nil }
                }
                .transition(.opacity.combined(with: .scale(scale: 0.9)))
            }
            Spacer()
            ReaderGlassButton(
                icon: "xmark",
                label: "Close book",
                ink: barInk,
                diameter: ReaderMenu.buttonSize
            ) {
                dismiss()
            }
        }
        .padding(.horizontal, ReaderMenu.inset)
        .padding(.top, Spacing.xs)
        .animation(Motion.snap, value: returnPoint)
    }

    /// The menu, and the one button that opens it.
    ///
    /// Everything the reader can do that isn't turning a page lives here, so
    /// the page itself carries a single control instead of a row of them.
    private var bottomBar: some View {
        VStack(alignment: .trailing, spacing: ReaderMenu.spacing) {
            if scrubFraction != nil {
                ReaderScrubReadout(
                    chapter: scrubChapter?.label,
                    page: scrubPage,
                    ink: barInk
                )
                .transition(.opacity.combined(with: .scale(scale: 0.96)))
            }

            if showMenu {
                menuRows
            }

            HStack {
                Spacer()
                if showMenu {
                    quickActions
                } else {
                    ReaderGlassButton(
                        icon: "line.3.horizontal",
                        label: "Menu",
                        ink: barInk,
                        diameter: ReaderMenu.buttonSize
                    ) {
                        withAnimation(Motion.lift) { showMenu = true }
                    }
                    .transition(.opacity.combined(with: .scale(scale: 0.9)))
                }
            }
        }
        .padding(.horizontal, ReaderMenu.inset)
        // Closed, the button shares the line with the page count. Open, the
        // whole stack lifts so the quick actions clear it rather than crowding
        // the one label that should always be readable.
        .padding(.bottom, showMenu ? 34 : Spacing.xs)
        .animation(Motion.snap, value: scrubFraction == nil)
    }

    /// The contents entry a scrub is currently pointing at.
    private var scrubChapter: TocItem? {
        guard let fraction = scrubFraction else { return nil }
        return controller.tocEntry(atFraction: fraction)
    }

    /// The page a scrub would land on. Whole-book page numbers only exist once
    /// epub.js's locations pass has run, so this is absent until then.
    private var scrubPage: Int? {
        guard let fraction = scrubFraction,
              let total = controller.location?.totalPages, total > 0
        else { return nil }
        return max(1, min(total, Int((fraction * Double(total)).rounded())))
    }

    @ViewBuilder
    private var menuRows: some View {
        ReaderScrubRow(
            fraction: controller.location?.fraction ?? 0,
            ink: barInk,
            onOpen: {
                closeMenu()
                contentsTab = .contents
                showContents = true
            },
            onScrubStart: markReturnPoint,
            onScrubChange: { scrubFraction = $0 },
            onSeek: { controller.seek(toFraction: $0) }
        )
        .transition(.opacity.combined(with: .move(edge: .bottom)))

        ReaderMenuRow(
            title: "Bookmarks & Highlights",
            count: bookmarkCount + highlights.count,
            ink: barInk
        ) {
            closeMenu()
            contentsTab = .bookmarks
            showContents = true
        }
        .transition(.opacity.combined(with: .move(edge: .bottom)))

        ReaderMenuRow(title: "Themes & Settings", icon: "textformat.size", ink: barInk) {
            closeMenu()
            showSettings = true
        }
        .transition(.opacity.combined(with: .move(edge: .bottom)))
    }

    private var quickActions: some View {
        HStack(spacing: ReaderMenu.spacing) {
            ReaderGlassButton(
                icon: "square.and.arrow.up", label: "Share", ink: barInk, diameter: 50
            ) {
                closeMenu()
                ShareSheet.present(items: [book.displayTitle])
            }
            ReaderGlassButton(
                icon: justBookmarked ? "bookmark.fill" : "bookmark",
                label: "Add bookmark",
                ink: barInk,
                diameter: 50
            ) {
                Task { await addBookmark() }
            }
        }
        .transition(.opacity.combined(with: .scale(scale: 0.9)))
    }

    private func closeMenu() {
        withAnimation(Motion.snap) { showMenu = false }
    }

    /// Remember where a scrub began so the return pill has somewhere to go.
    private func markReturnPoint() {
        guard let location = controller.location, let cfi = location.cfi else { return }
        returnPoint = ReturnPoint(cfi: cfi, page: location.page)
    }

    /// The bars sit on the page ground, not the app ground, so their ink has
    /// to follow the reading theme rather than the app theme.
    private var barInk: Color {
        ReaderTheme.ink(controller.settings.theme)
    }

    // MARK: - Lifecycle

    private func prepare() async {
        guard !didConfigure else { return }

        // Open from what this device already knows, with no network in the
        // path. Waiting on the server here is what made opening a book cost a
        // request timeout — three of them — every time it was unreachable, and
        // for a downloaded book there was nothing to wait for in the first
        // place.
        let local = await UserDataService.localProgress(uuid: book.uuid, format: .epub)
        openedProgress = local
        startCFI = local?.epubCFI
        highlights = await UserDataService.localHighlights(uuid: book.uuid)
        bookmarkCount = await UserDataService.localBookmarks(uuid: book.uuid).count

        // Nobody is reading yet, so a position from another device is not a
        // correction to offer — it is simply where this book is up to, and it
        // belongs in the first paint. Bounded, so an unreachable server still
        // costs nothing: past the deadline we open on what we have and the
        // banner handles anything that lands later.
        //
        // One read, waited on briefly and then handed to `reconcileWithServer`
        // to become the offer. Reading the same row twice cost a second request
        // per book open, and the deadline on the first was not a deadline at
        // all: cancelling that read unwinds through the outbox drain it
        // triggers, and joining a drain is not cancellable.
        let remote = Task { @MainActor in await newerRemotePosition() }
        if let settled = await firstResult(of: remote, within: PositionSync.openDeadline),
           settled != startCFI {
            startCFI = settled
        }

        controller.configure(book: book, startCFI: startCFI, highlights: highlights)
        didConfigure = true
        sessionStart = Date()

        LifecycleSync.shared.register(controller) {
            await persist(force: true)
            await checkpointSession()
        } resume: {
            // Only when the flush actually stopped the clock. A `.inactive`
            // blip — Control Centre, a notification banner, the app switcher
            // glimpsed — comes back through here without ever having
            // backgrounded, and restarting the clock there would throw away the
            // reading time before the interruption.
            if sessionStart == nil { sessionStart = Date() }
        }
        LifecycleSync.shared.didOpenBook()
        await reconcileWithServer(remote: remote)
    }

    /// The server's position for this book, when it is newer than the one this
    /// device opened on. Runs to completion whether or not anyone is still
    /// waiting on it.
    private func newerRemotePosition() async -> String? {
        await PositionSync.newerRemote(
            uuid: book.uuid, format: .epub, than: openedProgress
        )?.epubCFI?.nilIfBlank
    }

    /// Fold in whatever the server has that this device doesn't.
    ///
    /// Annotations merge silently — a highlight arriving late is additive and
    /// can't be wrong. A reading position can be, so it is never applied under
    /// the reader: if another device left off somewhere else *after* our last
    /// local write, we offer the jump and let the reader decide. Moving the
    /// page out from under someone mid-sentence is worse than being behind.
    private func reconcileWithServer(remote: Task<String?, Never>) async {
        await withTaskGroup(of: Void.self) { group in
            group.addTask { @MainActor in
                for await items in UserDataService.highlights(uuid: book.uuid).values() {
                    highlights = items
                    controller.applyHighlights(items)
                }
            }
            group.addTask { @MainActor in
                for await items in UserDataService.bookmarks(uuid: book.uuid).values() {
                    bookmarkCount = items.count
                }
            }
            group.addTask { @MainActor in
                // Beat the deadline and it is already the opening position, so
                // this is a no-op; landed after it and it becomes an offer.
                guard let settled = await remote.value, settled != startCFI else { return }
                withAnimation(Motion.settle) { syncOffer = settled }
            }
        }
    }

    /// Record where the reader is. `force` marks the moments that are worth a
    /// round trip of their own — a close, a backgrounding — as opposed to the
    /// steady trickle of page turns, which queue and go out with the next
    /// push. The position is durable either way; only its latency differs.
    private func persist(force: Bool) async {
        guard let cfi = controller.location?.cfi else { return }
        guard force || Date().timeIntervalSince(lastSave) > 4 else { return }
        lastSave = Date()
        await UserDataService.saveProgress(
            ProgressUpdate(bookUUID: book.uuid, format: .epub, epubCFI: cfi, audioPositionSeconds: nil),
            push: force
        )
        await checkpointSessionIfStale()
    }

    /// Cut the open reading session into a report every so often.
    ///
    /// Reporting only on background and close meant an afternoon's reading in
    /// one sitting was a single unreported span until the app changed phase —
    /// and an OOM kill or a crash took all of it. The player has always
    /// checkpointed on every pause; this gives the reader, which has no
    /// equivalent moment, a bound on what a lost process costs.
    private func checkpointSessionIfStale() async {
        guard let start = sessionStart,
              Date().timeIntervalSince(start) >= Self.sessionCheckpointInterval
        else { return }
        await checkpointSession(restarting: true)
    }

    /// Report the stretch of reading that just ended.
    ///
    /// Runs on the way to the background as well as on close, so the seconds
    /// either side of a backgrounding are two disjoint reports rather than one
    /// span that swallows however long the app was away. Each carries its own
    /// client id, so a report the outbox replays can't be counted twice.
    ///
    /// `restarting` opens the next window immediately, for the periodic
    /// checkpoint — closing without reopening there would stop counting a
    /// session that is still very much running.
    private func checkpointSession(restarting: Bool = false) async {
        guard let start = sessionStart else { return }
        let end = Date()
        sessionStart = restarting ? end : nil
        let elapsed = Int64(end.timeIntervalSince(start))
        guard elapsed >= 5 else {
            // Too short to be worth a row, but the clock has already moved —
            // give the seconds back rather than dropping them on a checkpoint.
            if restarting { sessionStart = start }
            return
        }
        await UserDataService.reportSessions([
            SessionReport(
                bookUUID: book.uuid,
                format: .epub,
                startedAt: Int64(start.timeIntervalSince1970),
                endedAt: Int64(end.timeIntervalSince1970),
                progressUnits: elapsed,
                deviceId: nil
            )
        ])
    }

    private func finish() async {
        LifecycleSync.shared.unregister(controller)
        await persist(force: true)
        await checkpointSession()
        controller.teardown()
        Presentation.shared.noteProgressPersisted()
        // Last, so the position and the session report are both queued before
        // the drain goes looking for them.
        LifecycleSync.shared.didCloseBook()
    }

    private func addBookmark() async {
        guard let cfi = controller.location?.cfi else { return }
        Haptics.success()
        withAnimation(Motion.snap) { justBookmarked = true }
        bookmarkCount += 1
        await UserDataService.createBookmark(
            CreateBookmark(
                bookUUID: book.uuid, position: cfi, title: controller.location?.chapterName
            )
        )
        // Long enough to read as a confirmation, short enough that the ribbon
        // isn't left claiming this page is permanently marked.
        try? await Task.sleep(for: .seconds(1.2))
        withAnimation(Motion.snap) { justBookmarked = false }
    }

    @discardableResult
    private func createHighlight(
        _ selection: SelectionData, color: HighlightColor
    ) async -> Highlight? {
        // Absent only while a drag is in flight, which is exactly when no menu
        // is up to have asked for this.
        guard let cfiRange = selection.cfiRange else { return nil }
        controller.addAnnotation(cfiRange: cfiRange, color: color)
        controller.clearSelection()
        Haptics.success()
        let created = await UserDataService.createHighlight(
            CreateHighlight(
                bookUUID: book.uuid,
                epubCFIRange: cfiRange,
                color: color,
                text: selection.text
            )
        )
        // Keep the local list authoritative so the passage is tappable
        // immediately, without waiting for a refetch.
        if let created {
            highlights.append(created)
        }
        return created
    }

    /// Noting a fresh selection highlights it first — a note has to hang on
    /// something, and Apple Books likewise turns "Note" into a highlight.
    private func createHighlightThenNote(_ selection: SelectionData) async {
        guard let created = await createHighlight(selection, color: .amber) else { return }
        noteTarget = created
    }

    private func recolor(_ highlight: Highlight, to color: HighlightColor) async {
        controller.tappedAnnotation = nil
        controller.removeAnnotation(cfiRange: highlight.epubCFIRange)
        controller.addAnnotation(
            cfiRange: highlight.epubCFIRange,
            color: color,
            hasNote: highlight.note?.nilIfBlank != nil
        )
        update(highlight) { $0.color = color }
        await UserDataService.setHighlightColor(highlight, color: color)
    }

    private func saveNote(_ note: String?, on highlight: Highlight) async {
        // Re-add the mark so the note underline appears or clears to match.
        controller.removeAnnotation(cfiRange: highlight.epubCFIRange)
        controller.addAnnotation(
            cfiRange: highlight.epubCFIRange,
            color: highlight.color,
            hasNote: note != nil
        )
        update(highlight) { $0.note = note }
        Haptics.success()
        await UserDataService.setHighlightNote(highlight, note: note)
    }

    private func removeHighlight(_ highlight: Highlight) async {
        controller.tappedAnnotation = nil
        controller.removeAnnotation(cfiRange: highlight.epubCFIRange)
        highlights.removeAll { $0.id == highlight.id }
        Haptics.warning()
        await UserDataService.deleteHighlight(highlight)
    }

    private func update(_ highlight: Highlight, _ change: (inout Highlight) -> Void) {
        guard let index = highlights.firstIndex(where: { $0.id == highlight.id }) else { return }
        change(&highlights[index])
    }
}

