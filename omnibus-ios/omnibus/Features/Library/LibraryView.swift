//  LibraryView.swift
//  The landing surface: a Continue-reading rail over a paginated cover grid.

import SwiftUI

@Observable
@MainActor
final class LibraryModel {
    var books: [Book] = []
    var resume: [ResumePoint] = []
    var shelves: [ShelfPreview] = []
    var isLoading = false
    var isLoadingMore = false
    var error: String?

    var sort: SortKey = .newestAdded {
        didSet { if sort != oldValue { Task { await reload() } } }
    }
    var direction: SortDirection = .desc {
        didSet { if direction != oldValue { Task { await reload() } } }
    }
    var formatFilter: Set<String> = [] {
        didSet { if formatFilter != oldValue { Task { await reload() } } }
    }
    /// The signed-in user's hidden-formats preference, seeded from
    /// `app.user` by the view. A change reshapes the grid like any filter.
    var hiddenFormats: [String] = [] {
        didSet { if hiddenFormats != oldValue { Task { await reload() } } }
    }
    /// First-page "N hidden" receipt for the section heading; `nil` when the
    /// viewer hides nothing.
    var hiddenCount: Int64?

    /// Header category strip. Format buckets are pushed to the server as a
    /// `formats` filter; `downloaded` is answered from the local library mirror,
    /// which is what lets it mean the whole library rather than whichever page
    /// happens to be loaded.
    var category: LibraryCategory = .all {
        didSet {
            guard category != oldValue else { return }
            Task { await reload() }
        }
    }

    /// What the grid renders. Every bucket is now resolved by the read itself,
    /// so this is the loaded page as-is.
    var visibleBooks: [Book] { books }

    /// The category's filter with the viewer's hidden-formats folded in —
    /// the one filter every page read uses.
    private var activeFilter: LibraryFilter {
        var filter = category.filter
        filter.hiddenFormats = hiddenFormats
        return filter
    }

    /// Background poll cadence, mirroring the web client's 60 s sync tick.
    static let pollInterval: Duration = .seconds(60)

    private var cursor: String?
    private var reachedEnd = false
    private var loadToken = UUID()
    /// Once the user has paged past the first screen, a background refresh
    /// would truncate the grid back to page one — so it stands down.
    private var hasPaginated = false

    func loadIfNeeded() async {
        guard books.isEmpty, !isLoading else { return }
        await reload()
    }

    func reload() async {
        let token = UUID()
        loadToken = token
        isLoading = true
        error = nil
        cursor = nil
        reachedEnd = false
        hasPaginated = false

        let sort = sort
        let direction = direction
        let filter = activeFilter

        await withTaskGroup(of: Void.self) { group in
            group.addTask { @MainActor in
                do {
                    for try await read in LibraryService.page(
                        sort: sort, direction: direction, filter: filter, cursor: nil
                    ) {
                        guard self.loadToken == token else { return }
                        self.books = read.value.books
                        self.cursor = read.value.nextCursor
                        self.reachedEnd = read.value.nextCursor == nil
                        self.hiddenCount = read.value.hiddenCount
                        self.error = nil
                        self.isLoading = false
                    }
                } catch {
                    guard self.loadToken == token, self.books.isEmpty else { return }
                    self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
                }
                if self.loadToken == token { self.isLoading = false }
            }
            group.addTask { @MainActor in
                for await points in UserDataService.recentProgress().values() {
                    guard self.loadToken == token else { return }
                    self.resume = points
                }
            }
            group.addTask { @MainActor in
                for await previews in UserDataService.shelfPreviews().values() {
                    guard self.loadToken == token else { return }
                    // Held unfiltered: which of these belong on the rail depends
                    // on who is signed in, and the identity can confirm after
                    // this read lands. `railShelves` decides at render.
                    self.shelves = previews
                }
            }
        }
    }

    /// Resume points only. Re-running the whole `reload` would refetch the
    /// grid and the shelves to update one card. `freshOnly` skips the replica
    /// yield so a background poll doesn't republish an unchanged rail.
    func refreshResume(freshOnly: Bool = false) async {
        do {
            for try await read in UserDataService.recentProgress() {
                guard read.isFresh || !freshOnly else { continue }
                resume = read.value
            }
        } catch {}
    }

    /// One background poll tick: pick up server-side changes (a finished
    /// import, progress from another device) without requiring a pull.
    /// Mirrors the web client's sync tick — the mirror sync self-throttles to
    /// one full pass per five minutes, and the first-page revalidation only
    /// repaints when the server's answer actually differs.
    func backgroundRefresh() async {
        guard Self.shouldPoll(
            isOnline: Connectivity.shared.isOnline,
            isLoading: isLoading,
            isLoadingMore: isLoadingMore,
            hasPaginated: hasPaginated
        ) else { return }

        async let mirror: Void = LibraryIndex.shared.sync()
        await refreshResume(freshOnly: true)
        await revalidateFirstPage()
        await mirror
    }

    /// Pure poll gate, split out so the guard is testable without a server:
    /// offline ticks are wasted requests, loading ticks would race the user,
    /// and a paginated grid must not be truncated back to page one.
    nonisolated static func shouldPoll(
        isOnline: Bool, isLoading: Bool, isLoadingMore: Bool, hasPaginated: Bool
    ) -> Bool {
        isOnline && !isLoading && !isLoadingMore && !hasPaginated
    }

    /// What the landing rail shows: your own shelves, minus a wishlist you have
    /// never put anything on.
    ///
    /// `GET /api/shelves` answers with every shelf you are *allowed* to see —
    /// other people's public ones, and for an admin everyone's. Those are still
    /// reachable through All; the rail is yours. A nil id means the identity
    /// hasn't confirmed yet (`setServer` reaches `.ready` before it does), and
    /// showing the superset for that moment beats blanking a rail that is about
    /// to be right.
    nonisolated static func railShelves(
        _ previews: [ShelfPreview], userId: Int64?
    ) -> [ShelfPreview] {
        previews.filter { preview in
            let isOwn = userId.map { preview.shelf.ownerUserId == $0 } ?? true
            let isUnusedWishlist = preview.shelf.kind == .wishlist && preview.shelf.bookCount == 0
            return isOwn && !isUnusedWishlist
        }
    }

    /// Re-reads the first page through the replica cache. `Cache.live` yields
    /// a fresh value only when the payload changed, so a quiet library never
    /// re-renders; a failed poll is invisible and the next tick retries.
    private func revalidateFirstPage() async {
        let token = loadToken
        do {
            for try await read in LibraryService.page(
                sort: sort, direction: direction, filter: activeFilter, cursor: nil
            ) {
                guard loadToken == token, !hasPaginated else { return }
                guard read.isFresh else { continue }
                books = read.value.books
                cursor = read.value.nextCursor
                reachedEnd = read.value.nextCursor == nil
                hiddenCount = read.value.hiddenCount
            }
        } catch {}
    }

    func loadMoreIfNeeded(currentItem: Book) async {
        guard !reachedEnd, !isLoadingMore, !isLoading, let cursor else { return }
        // Prefetch a page ahead of the end so the grid never shows a gap.
        guard let index = books.firstIndex(where: { $0.id == currentItem.id }),
              index >= books.count - 12
        else { return }

        isLoadingMore = true
        defer { isLoadingMore = false }
        do {
            // Pages past the first aren't cached, so this yields exactly once.
            for try await read in LibraryService.page(
                sort: sort, direction: direction, filter: activeFilter, cursor: cursor
            ) {
                let page = read.value
                let existing = Set(books.map(\.id))
                hasPaginated = true
                books.append(contentsOf: page.books.filter { !existing.contains($0.id) })
                self.cursor = page.nextCursor
                reachedEnd = page.nextCursor == nil || page.books.isEmpty
            }
        } catch {
            reachedEnd = true
        }
    }
}

struct LibraryView: View {
    @Binding var addSheetPresented: Bool

    init(addSheetPresented: Binding<Bool>) {
        _addSheetPresented = addSheetPresented
    }

    @Environment(\.palette) private var palette
    @Environment(AppState.self) private var app
    @State private var model = LibraryModel()
    @State private var path = NavigationPath()
    @Namespace private var bookZoom
    private var presentation = Presentation.shared

    // `alignment: .top` matters here: GridItem defaults to centering each
    // item within its row, so a row of `BookGridCell`s whose captions wrap to
    // a different number of lines (a book with a one-line title next to one
    // that wraps to two) gets its shorter cells pushed down to stay centered
    // — the cover art itself renders at the same 2:3 size, but sits at a
    // different vertical offset, which reads as "a different size" (#1481).
    private let columns = [GridItem(.adaptive(minimum: 112, maximum: 168), spacing: 16, alignment: .top)]

    /// Named so a test can prove it resolves. A misspelt SF Symbol doesn't fail
    /// to compile or warn — `Image(systemName:)` just draws nothing, so the
    /// button would ship as an empty circle.
    static let addGlyph = "plus.viewfinder"

    /// The library wears the colour of whatever you're currently reading.
    private var ambientTone: OKLCH? {
        model.resume.first.map { CoverIdentity($0.book).tone }
    }

    var body: some View {
        NavigationStack(path: $path) {
            Group {
                if model.isLoading && model.books.isEmpty {
                    LoadingView(label: "Loading your library")
                } else if let error = model.error, model.books.isEmpty {
                    ErrorStateView(message: error) { Task { await model.reload() } }
                } else if model.books.isEmpty {
                    EmptyStateView(
                        icon: "books.vertical",
                        title: "No books yet",
                        message: "Point the server at a library folder, or add a book from the You tab."
                    )
                } else {
                    content
                }
            }
            .background {
                ZStack(alignment: .top) {
                    ScreenBackground()
                    if let ambientTone {
                        AmbientWash(tone: ambientTone)
                            .frame(height: 420)
                            .frame(maxHeight: .infinity, alignment: .top)
                            .ignoresSafeArea(edges: .top)
                            // Cross-fades when the book you're reading changes.
                            .animation(Motion.page, value: ambientTone.h)
                    }
                }
            }
            // The masthead replaces the large title, so the bar would otherwise
            // reserve a second, empty header band above it.
            .toolbar(.hidden, for: .navigationBar)
            .topEdgeScrim()
            // An explicit pull is the one place the whole-library mirror is
            // worth re-pulling on demand rather than on its own schedule.
            .refreshTask {
                async let mirror: Void = LibraryIndex.shared.sync(force: true)
                await model.reload()
                await mirror
            }
            .withDestinations()
        }
        .environment(\.bookZoomNamespace, bookZoom)
        .task {
            // Seed the pref before the first read so page 1 already excludes;
            // a later identity refresh re-seeds via `onChange` below.
            model.hiddenFormats = app.user?.hiddenFormats ?? []
            await model.loadIfNeeded()
        }
        // An Account-screen save (or a login as someone else) reshapes the
        // already-mounted library tab.
        .onChange(of: app.user?.hiddenFormats) { _, formats in
            model.hiddenFormats = formats ?? []
        }
        // Background poll, mirroring the web client's sync tick. Tied to the
        // view's lifetime, so it pauses whenever the library isn't on screen.
        .task {
            // `Task.sleep` throws on cancellation — bail out then, rather
            // than letting a cancelled loop run one last refresh.
            while (try? await Task.sleep(for: LibraryModel.pollInterval)) != nil {
                await model.backgroundRefresh()
            }
        }
        // The reader and player are full-screen covers over the tab view, so
        // closing one neither re-runs `task` nor re-appears this view.
        .onChange(of: presentation.progressToken) { _, _ in
            Task { await model.refreshResume() }
        }
    }

    /// Continue, then shelves, then the collection. Everything else that used
    /// to sit here — a search pill, a category strip, a browse row — moved to
    /// the tab that owns it, so the landing is mostly books.
    private var content: some View {
        ScrollView {
            // Deliberately not a `LazyVStack`: it releases subviews that scroll
            // out of the viewport, and rebuilding one restores it with fresh
            // `@State`. The Continue carousel paid for that twice — it came back
            // reset to the first card, and every progress bar on it replayed its
            // draw-in animation. Laziness is only worth anything for the grid,
            // which is a `LazyVGrid` and manages its own.
            VStack(alignment: .leading, spacing: 30) {
                Masthead(title: "Library") {
                    HStack(spacing: Spacing.sm) {
                        OfflinePill()
                        addButton
                    }
                }

                if !model.resume.isEmpty {
                    ContinueHero(
                        points: Array(model.resume.prefix(5)),
                        // Only the rail is redrawn: an edit from here can't
                        // change which books are in progress, and a full reload
                        // would truncate the grid back to its first page.
                        onEdited: { Task { await model.refreshResume() } },
                        onOpenDetail: { path.append(Destination.book(uuid: $0.uuid)) }
                    )
                }

                if !railShelves.isEmpty {
                    ShelvesRail(previews: Array(railShelves.prefix(8))) {
                        path.append(Destination.shelves)
                    }
                }

                VStack(alignment: .leading, spacing: Spacing.md) {
                    sectionHeading
                    grid
                }
            }
            .padding(.bottom, 40)
        }
        .scrollIndicators(.hidden)
    }

    private var grid: some View {
        LazyVGrid(columns: columns, spacing: 26) {
            ForEach(Array(model.visibleBooks.enumerated()), id: \.element.id) { index, book in
                NavigationLink(value: Destination.book(uuid: book.uuid)) {
                    BookGridCell(book: book)
                }
                .buttonStyle(BookPressStyle())
                .bookContextMenu(book, onEdited: { Task { await model.reload() } })
                .bookZoomSource(book.uuid, in: bookZoom)
                .cascadeIn(index: index)
                .task { await model.loadMoreIfNeeded(currentItem: book) }
            }
        }
        .screenPadding()
        .overlay(alignment: .bottom) {
            if model.isLoadingMore {
                ProgressView()
                    .tint(palette.ink3Color)
                    .padding(.vertical, Spacing.lg)
                    .offset(y: 40)
            }
        }
    }

    /// The rail is "your shelves", so it answers to who is signed in as well as
    /// to what the server returned — and `app.user` can arrive after the read.
    private var railShelves: [ShelfPreview] {
        LibraryModel.railShelves(model.shelves, userId: app.user?.id)
    }

    /// The control sits here rather than in the masthead because this is the
    /// heading it acts on: sort, direction and category all reshape the grid
    /// directly below, and the label restates the category it's set to.
    private var sectionHeading: some View {
        HStack(spacing: Spacing.sm) {
            Text(model.category == .all ? "All books" : model.category.label)
                .font(.display(24))
                .foregroundStyle(palette.ink0Color)

            // The hidden-formats receipt: excluded books must never look
            // like data loss.
            if let hidden = model.hiddenCount, hidden > 0 {
                Text("\(hidden) hidden")
                    .font(.ui(12))
                    .foregroundStyle(palette.ink3Color)
            }

            Spacer(minLength: 0)

            filterMenu
        }
        .screenPadding()
    }

    /// Upload or scan, from the screen the new book will land on. The You tab
    /// keeps its own row into the same sheet — `MainTabView` owns the
    /// presentation, so both entrances are the one sheet rather than two.
    ///
    /// The glyph names both of the sheet's actions: viewfinder brackets are the
    /// camera affordance the barcode scanner needs to advertise, and the plus
    /// inside them keeps the upload. A bare plus hid the scanner behind a sheet
    /// nobody had a reason to open.
    ///
    /// The glyph carries the accent while the circle stays neutral: an
    /// accent-*filled* circle is how `filterMenu` says "a filter is on", and
    /// two filled circles side by side would read as one state.
    private var addButton: some View {
        Button {
            Haptics.tap()
            addSheetPresented = true
        } label: {
            Image(systemName: Self.addGlyph)
                .font(.system(size: 17, weight: .regular))
                .foregroundStyle(palette.accentColor)
                .frame(width: 32, height: 32)
                .background(Circle().fill(palette.bg2Color))
                .overlay(Circle().strokeBorder(palette.line2.color, lineWidth: 0.5))
        }
        .buttonStyle(PressableStyle())
        // Says what the sheet offers, since the glyph now implies a camera and
        // VoiceOver would otherwise announce only half of it.
        .accessibilityLabel("Add books by upload or barcode scan")
    }

    /// One control for everything that shapes the grid. Sort and format used to
    /// be a menu plus a pinned strip; both fit here without costing a band of
    /// the screen.
    private var filterMenu: some View {
        Menu {
            Picker("Show", selection: Binding(
                get: { model.category }, set: { model.category = $0 }
            )) {
                ForEach(LibraryCategory.allCases, id: \.self) { option in
                    Label(option.label, systemImage: option.icon).tag(option)
                }
            }

            Divider()

            Picker("Sort", selection: Binding(
                get: { model.sort }, set: { model.sort = $0 }
            )) {
                ForEach(SortKey.allCases, id: \.self) { Text($0.label).tag($0) }
            }

            Divider()

            Button {
                model.direction = model.direction.toggled
            } label: {
                Label(
                    model.direction == .asc ? "Ascending" : "Descending",
                    systemImage: model.direction == .asc ? "arrow.up" : "arrow.down"
                )
            }
        } label: {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(isFiltered ? palette.accentInk.color : palette.ink1Color)
                .frame(width: 32, height: 32)
                .background(
                    Circle().fill(isFiltered ? palette.accentColor : palette.bg2Color)
                )
                .overlay(
                    Circle().strokeBorder(palette.line2.color, lineWidth: 0.5)
                )
        }
        .animation(Motion.snap, value: isFiltered)
        .accessibilityLabel("Filter and sort")
    }

    /// Tints the control when it's actually doing something, so a filtered grid
    /// can't be mistaken for the whole library.
    private var isFiltered: Bool {
        model.category != .all || model.sort != .newestAdded || model.direction != .desc
    }
}

/// One cover in the grid.
///
/// Captions are free to be one or two lines: every cover renders into the same
/// 2:3 box, so cells in a row already share a baseline and a shorter caption
/// only shortens that cell rather than knocking the grid out of alignment.
struct BookGridCell: View {
    let book: Book

    init(book: Book) {
        self.book = book
    }

    @Environment(\.palette) private var palette
    private var downloads = DownloadManager.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            BookCover(identity: CoverIdentity(book))
                .coverShadow()
                .overlay(alignment: .bottomLeading) { badges }

            VStack(alignment: .leading, spacing: 1) {
                Text(book.displayTitle)
                    .font(.ui(12.5, weight: .medium))
                    .lineSpacing(0)
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)

                Text(book.authorDisplay)
                    .font(.ui(11))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// Offline state rides on the art rather than the caption, which keeps the
    /// text block to a predictable height.
    private var badges: some View {
        HStack(spacing: 4) {
            if downloads.isAnyDownloaded(book.uuid) {
                badge("arrow.down")
            }
        }
        .padding(6)
    }

    private func badge(_ icon: String) -> some View {
        Image(systemName: icon)
            .font(.system(size: 9, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: 19, height: 19)
            .background(Circle().fill(.black.opacity(0.55)))
            .background(Circle().fill(.ultraThinMaterial))
    }
}
