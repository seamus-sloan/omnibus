//  SearchView.swift
//  Live grouped search across books, authors, series, and tags.

import SwiftUI

/// Root of the Search tab. Carries no `NavigationStack` of its own — `SearchTab`
/// owns it, so results push onto the tab's history and keep swipe-back.
///
/// One scroll view holds the masthead, the field, and whatever the field is
/// currently producing. The field used to live in a `.searchable` bar drawer
/// under a stock large title, which made this the one tab that didn't look like
/// the app; the results likewise came out of an inset-grouped `List` and read
/// as Settings rather than as a library.
struct SearchView: View {
    @Environment(\.palette) private var palette
    @Environment(TabReselect.self) private var reselect

    @State private var query = ""
    @State private var results: PaletteResults?
    @State private var isSearching = false
    @State private var searchTask: Task<Void, Never>?

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 30) {
                VStack(spacing: 0) {
                    Masthead(title: "Search")
                    SearchField(text: $query, prompt: "Books, authors, series, tags")
                        .screenPadding()
                }

                content
            }
            .padding(.bottom, 40)
        }
        .scrollIndicators(.hidden)
        .scrollDismissesKeyboard(.immediately)
        .background(ScreenBackground())
        .toolbar(.hidden, for: .navigationBar)
        .topEdgeScrim()
        .onChange(of: query) { _, value in schedule(value) }
        // Re-tapping the Search tab returns to the plain browse page, so the
        // query clears alongside the stack `SearchTab` unwinds.
        .onChange(of: reselect.token) { _, _ in
            guard reselect.tab == .search else { return }
            query = ""
        }
    }

    @ViewBuilder
    private var content: some View {
        if trimmed.isEmpty {
            BrowseDirectory()
        } else if let results, results.isEmpty, !isSearching {
            EmptyStateView(
                icon: "questionmark.circle",
                title: "No matches",
                message: "Nothing in the library matches “\(query)”."
            )
        } else if let results {
            resultSections(results)
        } else {
            LoadingView()
        }
    }

    // MARK: - Results

    private func resultSections(_ results: PaletteResults) -> some View {
        VStack(alignment: .leading, spacing: 30) {
            if !results.books.isEmpty {
                section("Books", total: results.bookTotal, shown: results.books.count) {
                    ForEach(Array(results.books.enumerated()), id: \.element.id) { index, hit in
                        NavigationLink(value: Destination.book(uuid: hit.uuid)) {
                            bookRow(hit, isFirst: index == 0)
                        }
                        .buttonStyle(PressableStyle())
                    }
                    RecordRule()
                }
            }

            if !results.authors.isEmpty {
                section("Authors", total: results.authorTotal, shown: results.authors.count) {
                    ForEach(Array(results.authors.enumerated()), id: \.element.id) { index, hit in
                        NavigationLink(value: Destination.author(id: hit.id)) {
                            personRow(
                                id: hit.id, name: hit.name,
                                caption: caption(count: hit.bookCount, lead: hit.leadBookTitle),
                                isFirst: index == 0
                            )
                        }
                        .buttonStyle(PressableStyle())
                    }
                    RecordRule()
                }
            }

            if !results.series.isEmpty {
                section("Series", total: results.seriesTotal, shown: results.series.count) {
                    ForEach(Array(results.series.enumerated()), id: \.element.id) { index, hit in
                        NavigationLink(value: Destination.series(id: hit.id)) {
                            plainRow(
                                title: hit.name,
                                caption: caption(count: hit.bookCount, lead: hit.leadBookTitle),
                                isFirst: index == 0
                            )
                        }
                        .buttonStyle(PressableStyle())
                    }
                    RecordRule()
                }
            }

            // Tags are short labels with a count — as full-width rows they read
            // as a list of almost nothing. A cloud shows the whole set at once.
            if !results.tags.isEmpty {
                VStack(alignment: .leading, spacing: Spacing.md) {
                    SectionLabel("Tags")
                    FlowLayout(spacing: 6, lineSpacing: 6) {
                        ForEach(results.tags) { hit in
                            NavigationLink(value: Destination.tag(name: hit.name)) {
                                Chip(label: hit.name, count: Int(hit.bookCount))
                            }
                            .buttonStyle(PressableStyle())
                        }
                    }
                }
                .screenPadding()
            }
        }
    }

    private func section<Content: View>(
        _ title: String, total: UInt32, shown: Int, @ViewBuilder rows: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            HStack(alignment: .firstTextBaseline) {
                SectionLabel(title)
                Spacer(minLength: Spacing.sm)
                if Int(total) > shown {
                    NavigationLink(value: Destination.searchResults(query: trimmed)) {
                        HStack(spacing: 3) {
                            Text("All \(total)")
                            Image(systemName: "chevron.right")
                                .font(.system(size: 10, weight: .semibold))
                        }
                        .font(.ui(13, weight: .medium))
                        .foregroundStyle(palette.accentColor)
                    }
                }
            }

            VStack(spacing: 0) { rows() }
        }
        .screenPadding()
    }

    private func bookRow(_ hit: PaletteBookHit, isFirst: Bool) -> some View {
        rowShell(isFirst: isFirst) {
            BookCover(
                identity: CoverIdentity(uuid: hit.uuid, title: hit.title, hasCover: hit.coverURL != nil),
                size: .sm
            )
            .frame(width: 38)
            .coverShadow(0.6)

            VStack(alignment: .leading, spacing: 2) {
                Text(hit.title)
                    .font(.ui(14.5, weight: .medium))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                HStack(spacing: 5) {
                    Text(hit.authorDisplay).lineLimit(1)
                    if let year = hit.year { Text("· \(year)") }
                }
                .font(.ui(11.5))
                .foregroundStyle(palette.ink3Color)
            }

            Spacer(minLength: 0)
            chevron
        }
    }

    private func personRow(id: Int64, name: String, caption: String, isFirst: Bool) -> some View {
        rowShell(isFirst: isFirst) {
            AuthorAvatar(id: id, name: name, hasPhoto: false, size: 34)
            labelStack(title: name, caption: caption)
            Spacer(minLength: 0)
            chevron
        }
    }

    private func plainRow(title: String, caption: String, isFirst: Bool) -> some View {
        rowShell(isFirst: isFirst) {
            labelStack(title: title, caption: caption)
            Spacer(minLength: 0)
            chevron
        }
    }

    private func rowShell<Content: View>(
        isFirst: Bool, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }
            HStack(spacing: Spacing.md) {
                content()
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
    }

    private func labelStack(title: String, caption: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.ui(14.5, weight: .medium))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(1)
            Text(caption)
                .font(.ui(11.5))
                .foregroundStyle(palette.ink3Color)
                .lineLimit(1)
        }
    }

    private var chevron: some View {
        Image(systemName: "chevron.right")
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(palette.ink3Color.opacity(0.7))
    }

    private func caption(count: UInt32, lead: String?) -> String {
        let books = "\(count) book\(count == 1 ? "" : "s")"
        guard let lead else { return books }
        return "\(books) · incl. \(lead)"
    }

    /// Restart the search on every keystroke. The service answers from the
    /// local mirror straight away and from the server after its own debounce,
    /// so cancelling here also cancels a server read the reader has already
    /// typed past.
    private func schedule(_ value: String) {
        searchTask?.cancel()
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            results = nil
            return
        }
        searchTask = Task {
            isSearching = true
            defer { isSearching = false }
            for await answer in LibraryService.searchPalette(query: trimmed) {
                guard !Task.isCancelled else { return }
                results = answer
            }
        }
    }
}

/// Full result grid for one query — reached from "See all" and from tag taps.
struct SearchResultsView: View {
    let query: String
    let title: String

    init(query: String, title: String) {
        self.query = query
        self.title = title
    }

    @Environment(\.palette) private var palette
    @State private var books: [Book] = []
    @State private var isLoading = true

    private let columns = [GridItem(.adaptive(minimum: 104, maximum: 150), spacing: 14)]

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else if books.isEmpty {
                EmptyStateView(icon: "questionmark.circle", title: "No matches")
            } else {
                ScrollView {
                    LazyVGrid(columns: columns, spacing: 20) {
                        ForEach(Array(books.enumerated()), id: \.element.id) { index, book in
                            NavigationLink(value: Destination.book(uuid: book.uuid)) {
                                BookGridCell(book: book)
                            }
                            .buttonStyle(BookPressStyle())
                            .cascadeIn(index: index)
                        }
                    }
                    .screenPadding()
                    .padding(.vertical, Spacing.lg)
                }
                .scrollIndicators(.hidden)
            }
        }
        .background(ScreenBackground())
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            for await hits in LibraryService.searchFull(query: query) {
                books = hits
                isLoading = false
            }
            isLoading = false
        }
    }
}
