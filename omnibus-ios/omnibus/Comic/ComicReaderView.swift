//  ComicReaderView.swift
//  The native CBZ pager: a paged TabView of zoomable pages inside the
//  reader's chrome contract — bare on open, close button and a page slider
//  one centre tap away, the same indicator labels above and below the page.
//
//  The EPUB reader's WKWebView + epub.js host stays EPUB-only; comics are
//  images, so the platform pager and scroll view do everything the web
//  stack was carrying. Positions ride the same Epub-format progress row as
//  every other reader via `ComicPosition`, so Continue Reading and
//  cross-device sync need no comic-specific server state.

import SwiftUI

struct ComicReaderView: View {
    let book: Book

    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) private var appState
    @Environment(AudioPlayer.self) private var audio

    /// The page source, once the archive or the detail read has resolved.
    /// Doubles as the LifecycleSync registration token.
    @State private var pages: ComicPages?
    @State private var failureMessage: String?
    @State private var page = 0
    @State private var chromeVisible = false
    /// The record the pager opened on, so a later server answer can be
    /// judged newer or older than what this device knew.
    @State private var openedProgress: ProgressRecord?
    /// A further page another device reached, waiting on the reader to
    /// accept it. Never applied on its own.
    @State private var syncOfferPage: Int?
    /// When the current stretch of reading began, or `nil` while the app is
    /// in the background — same reading-time clock as the EPUB reader.
    @State private var sessionStart: Date?
    @State private var lastSave = Date.distantPast
    @State private var showPlayer = false

    /// How long a single stretch of reading may run before it is reported
    /// as its own session rather than held until the book closes.
    private static let sessionCheckpointInterval: TimeInterval = 300

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()

            if let pages {
                pager(pages)
            } else if failureMessage == nil {
                LoadingView(label: "Opening \(book.displayTitle)")
            }

            if let failureMessage {
                ErrorStateView(message: failureMessage) { dismiss() }
            }

            indicators

            if let offered = syncOfferPage {
                SyncOfferBanner(
                    onGo: {
                        withAnimation(Motion.page) { page = offered }
                        dismissSyncOffer()
                    },
                    onDismiss: dismissSyncOffer
                )
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            chrome
        }
        .statusBarHidden(!chromeVisible)
        .persistentSystemOverlays(chromeVisible ? .automatic : .hidden)
        // The stage is black whatever the app theme, so the status bar and
        // any glass chrome have to resolve against a dark page.
        .preferredColorScheme(.dark)
        .task { await prepare() }
        .onChange(of: page) { _, newPage in
            Task {
                await pages?.prefetch(around: newPage)
                await persist(force: false)
            }
        }
        .onChange(of: audio.isActive) { _, active in
            if !active { showPlayer = false }
        }
        .onDisappear {
            Task { await finish() }
        }
        // The audio dock: playback stays controllable while reading, same
        // as the EPUB reader's immersive-read contract.
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if audio.isActive {
                    MiniPlayerBar(onExpand: { showPlayer = true })
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(Motion.glide, value: audio.isActive)
        }
        .sheet(isPresented: $showPlayer) {
            if let audioBook = audio.book {
                PlayerView(book: audioBook, fileID: audio.fileID)
                    .preferredColorScheme(appState.theme.colorScheme)
            }
        }
    }

    // MARK: - Pager

    private func pager(_ pages: ComicPages) -> some View {
        TabView(selection: $page) {
            ForEach(0..<pages.count, id: \.self) { index in
                ComicPageCell(index: index, pages: pages, onTap: handleTap)
                    .tag(index)
            }
        }
        .tabViewStyle(.page(indexDisplayMode: .never))
        .ignoresSafeArea()
    }

    private func handleTap(_ zone: ComicTapZone) {
        switch zone {
        case .previous:
            guard page > 0 else { return }
            withAnimation(Motion.page) { page -= 1 }
        case .next:
            guard let pages, page < pages.count - 1 else { return }
            withAnimation(Motion.page) { page += 1 }
        case .toggle:
            withAnimation(Motion.settle) { chromeVisible.toggle() }
        }
    }

    // MARK: - Chrome

    /// The two centred labels: the book above, the page below — the page
    /// count joining in while the chrome (navigation, not reading) is up.
    /// Same bands and type as the EPUB reader's `ReaderIndicatorLabels`.
    private var indicators: some View {
        VStack(spacing: 0) {
            indicatorLabel(pages == nil ? nil : book.displayTitle)
                .padding(.top, Spacing.xs)
            Spacer(minLength: 0)
            indicatorLabel(pageLabel)
                .padding(.bottom, Spacing.xs)
        }
        .allowsHitTesting(false)
    }

    private var pageLabel: String? {
        guard let pages, pages.count > 0 else { return nil }
        return chromeVisible ? "\(page + 1) of \(pages.count)" : "\(page + 1)"
    }

    @ViewBuilder
    private func indicatorLabel(_ text: String?) -> some View {
        if let text {
            Text(text)
                .font(.ui(12.5))
                .foregroundStyle(.white.opacity(0.5))
                .lineLimit(1)
                .frame(height: ReaderMenu.buttonSize)
                .padding(.horizontal, 72)
        }
    }

    @ViewBuilder
    private var chrome: some View {
        if chromeVisible {
            VStack {
                HStack {
                    Spacer()
                    ReaderGlassButton(
                        icon: "xmark",
                        label: "Close book",
                        ink: .white,
                        diameter: ReaderMenu.buttonSize
                    ) {
                        dismiss()
                    }
                }
                .padding(.horizontal, ReaderMenu.inset)
                .padding(.top, Spacing.xs)

                Spacer()

                if let pages, pages.count > 1 {
                    pageSlider(pages)
                        .padding(.bottom, 34)
                }
            }
            .transition(.opacity)
        }
    }

    /// The whole-book scrubber: one slider row, page-stepped. Comics have no
    /// chapters or locations pass, so unlike the EPUB reader the range is
    /// exact from the first paint.
    private func pageSlider(_ pages: ComicPages) -> some View {
        Slider(
            value: Binding(
                get: { Double(page) },
                set: { page = Int($0.rounded()) }
            ),
            in: 0...Double(pages.count - 1),
            step: 1
        )
        .tint(.white.opacity(0.85))
        .padding(.horizontal, 18)
        .frame(width: ReaderMenu.width, height: ReaderMenu.rowHeight)
        .glassEffect(.regular, in: .capsule)
        .accessibilityLabel("Page")
        .accessibilityValue("Page \(page + 1) of \(pages.count)")
    }

    /// Declining is as final as accepting — re-offering the same page on the
    /// next turn would nag through the whole book.
    private func dismissSyncOffer() {
        withAnimation(Motion.settle) { syncOfferPage = nil }
    }

    // MARK: - Lifecycle

    private func prepare() async {
        guard pages == nil, failureMessage == nil else { return }

        // Open from what this device already knows, with no network in the
        // path — a downloaded comic must open with the server gone.
        let local = await UserDataService.localProgress(uuid: book.uuid, format: .epub)
        openedProgress = local

        let source = await resolveSource()
        guard let source else { return }
        guard source.count > 0 else {
            failureMessage = "This comic has no pages."
            return
        }

        var start = ComicPosition.startPage(
            anchor: local?.epubCFI,
            percent: local?.progressPercent,
            count: source.count
        )

        // A position from another device, folded into the first paint when
        // it answers within the deadline and offered as a jump when it
        // lands later — the EPUB reader's open contract.
        let remote = Task { @MainActor in
            await PositionSync.newerRemote(uuid: book.uuid, format: .epub, than: local)
        }
        if let settled = await firstResult(of: remote, within: PositionSync.openDeadline) {
            start = resumePage(of: settled, count: source.count)
        }

        page = start
        pages = source
        sessionStart = Date()
        await source.prefetch(around: start)

        LifecycleSync.shared.register(source) {
            await persist(force: true)
            await checkpointSession()
        } resume: {
            // Only when the flush actually stopped the clock — an
            // `.inactive` blip comes back through here without ever having
            // backgrounded.
            if sessionStart == nil { sessionStart = Date() }
        }
        LifecycleSync.shared.didOpenBook()

        guard let record = await remote.value else { return }
        let offered = resumePage(of: record, count: source.count)
        if offered != page {
            withAnimation(Motion.settle) { syncOfferPage = offered }
        }
    }

    /// The downloaded archive when this book is on the device, else the
    /// server's per-page endpoint sized by the detail read's `page_count`.
    private func resolveSource() async -> ComicPages? {
        if let local = DownloadManager.shared.localURL(for: book.uuid, kind: .ebook),
           local.pathExtension.lowercased() == "cbz"
        {
            do {
                return try ComicPages(localURL: local)
            } catch {
                // A damaged download must not dead-end the book while the
                // server can still serve it page by page.
                guard Connectivity.shared.isOnline else {
                    failureMessage = error.localizedDescription
                    return nil
                }
            }
        }
        if let count = book.pageCount, count > 0 {
            return ComicPages(uuid: book.uuid, count: Int(count))
        }
        // Opened from a surface whose payload has no page count (the resume
        // rail's projection) — the detail read carries it.
        if let detail = try? await LibraryService.settledBook(uuid: book.uuid),
           let count = detail.pageCount, count > 0
        {
            return ComicPages(uuid: book.uuid, count: Int(count))
        }
        failureMessage = "This comic couldn't be opened."
        return nil
    }

    /// The page a saved record resumes at, via the shared anchor-then-percent
    /// fallback.
    private func resumePage(of record: ProgressRecord, count: Int) -> Int {
        ComicPosition.startPage(
            anchor: record.epubCFI,
            percent: record.progressPercent,
            count: count
        )
    }

    /// Record where the reader is. `force` marks the moments worth a round
    /// trip of their own — a close, a backgrounding — while page turns queue
    /// and go out with the next push, exactly like the EPUB reader's CFI
    /// trickle.
    private func persist(force: Bool) async {
        guard let pages, pages.count > 0 else { return }
        guard force || Date().timeIntervalSince(lastSave) > 4 else { return }
        lastSave = Date()
        let clamped = min(max(page, 0), pages.count - 1)
        await UserDataService.saveProgress(
            ProgressUpdate(
                bookUUID: book.uuid,
                format: .epub,
                epubCFI: ComicPosition.anchor(page: clamped),
                audioPositionSeconds: nil,
                progressPercent: ComicPosition.percent(page: clamped, count: pages.count)
            ),
            push: force
        )
        await checkpointSessionIfStale()
    }

    private func checkpointSessionIfStale() async {
        guard let start = sessionStart,
              Date().timeIntervalSince(start) >= Self.sessionCheckpointInterval
        else { return }
        await checkpointSession(restarting: true)
    }

    /// Report the stretch of reading that just ended — same rules as the
    /// EPUB reader: disjoint spans across backgroundings, own client id per
    /// report, sub-5-second stretches returned to the clock.
    private func checkpointSession(restarting: Bool = false) async {
        guard let start = sessionStart else { return }
        let end = Date()
        sessionStart = restarting ? end : nil
        let elapsed = Int64(end.timeIntervalSince(start))
        guard elapsed >= 5 else {
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
        if let pages { LifecycleSync.shared.unregister(pages) }
        await persist(force: true)
        await checkpointSession()
        Presentation.shared.noteProgressPersisted()
        // Last, so the position and the session report are both queued
        // before the drain goes looking for them.
        LifecycleSync.shared.didCloseBook()
    }
}

/// One page of the pager: spinner while the bytes come, the zoomable image
/// once decoded, and a tappable retry when a fetch fails.
private struct ComicPageCell: View {
    let index: Int
    let pages: ComicPages
    let onTap: (ComicTapZone) -> Void

    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack {
            if let image {
                ComicPageView(image: image, onTap: onTap)
            } else if failed {
                ErrorStateView(message: "This page couldn't be loaded.") {
                    failed = false
                    Task { await load() }
                }
            } else {
                ProgressView()
                    .controlSize(.large)
                    .tint(.white.opacity(0.6))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.black)
        .task(id: index) { await load() }
    }

    private func load() async {
        guard image == nil else { return }
        do {
            let bytes = try await pages.page(index)
            guard let decoded = UIImage(data: bytes) else {
                failed = true
                return
            }
            // Decode off the render path — a multi-megapixel page decoded
            // lazily at draw time stutters the turn that revealed it.
            image = await decoded.byPreparingForDisplay() ?? decoded
        } catch {
            failed = true
        }
    }
}
