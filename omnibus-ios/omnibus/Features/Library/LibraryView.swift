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
    var total: Int64?

    var sort: SortKey = .newestAdded {
        didSet { if sort != oldValue { Task { await reload() } } }
    }
    var direction: SortDirection = .desc {
        didSet { if direction != oldValue { Task { await reload() } } }
    }
    var formatFilter: Set<String> = [] {
        didSet { if formatFilter != oldValue { Task { await reload() } } }
    }

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

    private var cursor: String?
    private var reachedEnd = false
    private var loadToken = UUID()

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

        let sort = sort
        let direction = direction
        let filter = category.filter

        await withTaskGroup(of: Void.self) { group in
            group.addTask { @MainActor in
                do {
                    for try await read in LibraryService.page(
                        sort: sort, direction: direction, filter: filter, cursor: nil
                    ) {
                        guard self.loadToken == token else { return }
                        self.books = read.value.books
                        self.cursor = read.value.nextCursor
                        self.total = read.value.total
                        self.reachedEnd = read.value.nextCursor == nil
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
                    // Only the user's own shelves belong on the landing rail; a
                    // public shelf from another account is browsable but isn't
                    // "yours".
                    self.shelves = previews.filter {
                        $0.shelf.bookCount > 0 || $0.shelf.kind != .wishlist
                    }
                }
            }
        }
    }

    /// Resume points only. Re-running the whole `reload` would refetch the
    /// grid and the shelves to update one card.
    func refreshResume() async {
        for await points in UserDataService.recentProgress().values() { resume = points }
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
                sort: sort, direction: direction, filter: category.filter, cursor: cursor
            ) {
                let page = read.value
                let existing = Set(books.map(\.id))
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

    private let columns = [GridItem(.adaptive(minimum: 112, maximum: 168), spacing: 16)]

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
            .refreshable {
                async let mirror: Void = LibraryIndex.shared.sync(force: true)
                await model.reload()
                await mirror
            }
            .withDestinations()
        }
        .environment(\.bookZoomNamespace, bookZoom)
        .task { await model.loadIfNeeded() }
        // The reader and player are full-screen covers over the tab view, so
        // closing one neither re-runs `task` nor re-appears this view.
        .onChange(of: presentation.progressToken) { _, _ in
            Task { await model.refreshResume() }
        }
    }

    /// Continue, then shelves, then the collection. Everything else that used
    /// to sit here — a search pill, an add button, a category strip, a browse
    /// row — moved to the tab that owns it, so the landing is mostly books.
    private var content: some View {
        ScrollView {
            // Deliberately not a `LazyVStack`: it releases subviews that scroll
            // out of the viewport, and rebuilding one restores it with fresh
            // `@State`. The Continue carousel paid for that twice — it came back
            // reset to the first card, and every progress bar on it replayed its
            // draw-in animation. Laziness is only worth anything for the grid,
            // which is a `LazyVGrid` and manages its own.
            VStack(alignment: .leading, spacing: 30) {
                Masthead(title: "Library", count: countLabel) {
                    HStack(spacing: Spacing.sm) {
                        OfflinePill()
                        filterMenu
                    }
                }

                if !model.resume.isEmpty {
                    ContinueHero(points: Array(model.resume.prefix(5)))
                }

                if !model.shelves.isEmpty {
                    ShelvesRail(previews: Array(model.shelves.prefix(8))) {
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

    private var sectionHeading: some View {
        HStack(alignment: .firstTextBaseline, spacing: Spacing.sm) {
            Text(model.category == .all ? "All books" : model.category.label)
                .font(.display(24))
                .foregroundStyle(palette.ink0Color)

            Spacer(minLength: 0)
        }
        .screenPadding()
    }

    private var countLabel: String {
        let shown = model.visibleBooks.count
        if let total = model.total, model.category == .all, Int(total) != shown {
            return "\(total)"
        }
        return "\(shown)"
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

    /// Format and offline state ride on the art rather than the caption, which
    /// keeps the text block to a predictable height.
    private var badges: some View {
        HStack(spacing: 4) {
            if book.hasAudiobook {
                badge("headphones")
            }
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
