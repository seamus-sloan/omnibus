//  DiscoveryViews.swift
//  Authors index + detail, series index + detail, and the tag cloud.

import SwiftUI

// MARK: - Authors

struct AuthorsView: View {
    @Environment(\.palette) private var palette
    @State private var authors: [AuthorSummary] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var query = ""

    private var filtered: [AuthorSummary] {
        guard let needle = query.nilIfBlank?.lowercased() else { return authors }
        return authors.filter { $0.name.lowercased().contains(needle) }
    }

    /// Grouped by initial, in order.
    ///
    /// The server returns authors in its own order, which on a shelf of any
    /// size reads as no order at all — you scroll looking for a name with no
    /// idea whether you have already passed it.
    ///
    /// Ordering follows the *displayed* name rather than the server's `sort`
    /// field. That field is populated inconsistently — some rows carry
    /// "Alcott, Louisa May" and others "Ada Lovelace" — so indexing on it put
    /// half the library under the surname's letter and half under the given
    /// name's, with the avatar initial disagreeing with the section it sat in.
    /// One letter, always the one you can see in the name, is the property that
    /// makes an index usable.
    private var sections: [(letter: String, authors: [AuthorSummary])] {
        let sorted = filtered.sorted {
            $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
        return Dictionary(grouping: sorted, by: initial)
            .sorted { lhs, rhs in
                // Anything that doesn't start with a letter files under "#",
                // which belongs at the end rather than sorted as punctuation.
                if lhs.key == "#" { return false }
                if rhs.key == "#" { return true }
                return lhs.key < rhs.key
            }
            .map { (letter: $0.key, authors: $0.value) }
    }

    private func initial(_ author: AuthorSummary) -> String {
        guard let first = author.name.first, first.isLetter else { return "#" }
        return String(first).uppercased()
    }

    /// Pushed as a destination, never a tab root — so it must not carry its own
    /// `NavigationStack`. It did, left over from when Authors was a tab, and a
    /// stack nested inside a stack silently swallows the push: tapping Authors
    /// in Search did nothing while Series (which never had one) worked.
    /// `withDestinations` is likewise the enclosing stack's job.
    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else if let error, authors.isEmpty {
                ErrorStateView(message: error) { Task { await load() } }
            } else if authors.isEmpty {
                EmptyStateView(icon: "person.2", title: "No authors yet")
            } else if filtered.isEmpty {
                EmptyStateView(
                    icon: "questionmark.circle",
                    title: "No authors match",
                    message: "Nothing in the library is filed under “\(query)”."
                )
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: Spacing.lg, pinnedViews: .sectionHeaders) {
                        ForEach(sections, id: \.letter) { section in
                            Section {
                                VStack(spacing: 0) {
                                    ForEach(Array(section.authors.enumerated()), id: \.element.id) { index, author in
                                        NavigationLink(value: Destination.author(id: author.id)) {
                                            AuthorRow(author: author, isFirst: index == 0)
                                        }
                                        .buttonStyle(PressableStyle())
                                    }
                                    RecordRule()
                                }
                                .screenPadding()
                            } header: {
                                letterHeader(section.letter)
                            }
                        }
                    }
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Authors")
        .searchable(text: $query, prompt: "Filter authors")
        .refreshTask { await load(force: true) }
        .task { await load() }
    }

    /// The index rail. Pinned so you always know where you are in the alphabet.
    private func letterHeader(_ letter: String) -> some View {
        HStack(spacing: Spacing.md) {
            Text(letter)
                .font(.display(15, weight: .semibold))
                .foregroundStyle(palette.accentColor)
                .frame(width: 16, alignment: .leading)

            Rectangle()
                .fill(palette.line2.color)
                .frame(height: 0.5)
        }
        .padding(.vertical, 6)
        .screenPadding()
        .background(palette.bg0Color)
    }

    private func load(force: Bool = false) async {
        if force { await OfflineStore.shared.cacheDelete(CacheKey.authors) }
        do {
            for try await read in LibraryService.authors() {
                authors = read.value
                error = nil
                isLoading = false
            }
        } catch {
            if authors.isEmpty {
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
        isLoading = false
    }
}

private struct AuthorRow: View {
    let author: AuthorSummary
    var isFirst = false

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                AuthorAvatar(id: author.id, name: author.name, hasPhoto: author.hasPhoto, size: 40)

                VStack(alignment: .leading, spacing: 2) {
                    Text(author.name)
                        .font(.ui(15, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(1)
                    Text("\(author.bookCount) book\(author.bookCount == 1 ? "" : "s")")
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                }

                Spacer(minLength: 0)

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.ink3Color.opacity(0.7))
            }
            .padding(.vertical, 9)
            .contentShape(Rectangle())
        }
    }
}

struct AuthorAvatar: View {
    let id: Int64
    let name: String
    let hasPhoto: Bool
    var size: CGFloat = 42

    @Environment(\.palette) private var palette

    var body: some View {
        Group {
            if hasPhoto {
                RemoteImage(path: "/api/authors/\(id)/photo") { letter }
            } else {
                letter
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(palette.line2.color, lineWidth: 0.5))
    }

    private var letter: some View {
        ZStack {
            palette.bg3Color
            Text(name.prefix(1).uppercased())
                .font(.display(size * 0.42, weight: .semibold))
                .foregroundStyle(palette.ink2Color)
        }
    }
}

struct AuthorDetailView: View {
    let id: Int64

    init(id: Int64) {
        self.id = id
    }

    @Environment(\.palette) private var palette
    @State private var detail: AuthorDetail?
    @State private var isLoading = true
    @State private var error: String?
    @State private var scrollY: CGFloat = 0

    // `.top` keeps `BookGridCell`s aligned by their cover art when captions
    // in the same row wrap to different line counts — see LibraryView.swift.
    private let columns = [GridItem(.adaptive(minimum: 104, maximum: 150), spacing: 14, alignment: .top)]

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else if let detail {
                ScrollView {
                    VStack(alignment: .leading, spacing: Spacing.xl) {
                        HStack(spacing: Spacing.lg) {
                            AuthorAvatar(
                                id: detail.id, name: detail.name,
                                hasPhoto: detail.hasPhoto, size: 76
                            )
                            VStack(alignment: .leading, spacing: 4) {
                                Text(detail.name)
                                    .font(.display(27))
                                    .foregroundStyle(palette.ink0Color)
                                Text("\(detail.bookCount) book\(detail.bookCount == 1 ? "" : "s")")
                                    .font(.ui(13))
                                    .foregroundStyle(palette.ink3Color)
                            }
                            Spacer(minLength: 0)
                        }

                        LazyVGrid(columns: columns, spacing: 20) {
                            ForEach(Array(detail.books.enumerated()), id: \.element.id) { index, book in
                                NavigationLink(value: Destination.book(uuid: book.uuid)) {
                                    BookGridCell(book: book)
                                }
                                .buttonStyle(BookPressStyle())
                                .cascadeIn(index: index)
                            }
                        }
                    }
                    .screenPadding()
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
                .trackingScrollOffset($scrollY)
            } else if let error {
                ErrorStateView(message: error) { Task { await load() } }
            }
        }
        .background(ScreenBackground())
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            FadingBarTitle(title: detail?.name ?? "", scrollY: scrollY, appearsAfter: 48)
        }
        .task { await load() }
    }

    private func load() async {
        do {
            for try await read in LibraryService.author(id: id) {
                detail = read.value
                error = nil
                isLoading = false
            }
        } catch {
            if detail == nil {
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
        isLoading = false
    }
}

// MARK: - Series

struct SeriesIndexView: View {
    @Environment(\.palette) private var palette
    @State private var series: [SeriesSummary] = []
    @State private var isLoading = true
    @State private var query = ""

    private var filtered: [SeriesSummary] {
        let matched: [SeriesSummary]
        if let needle = query.nilIfBlank?.lowercased() {
            matched = series.filter { $0.name.lowercased().contains(needle) }
        } else {
            matched = series
        }
        return matched.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else if series.isEmpty {
                EmptyStateView(icon: "square.grid.2x2", title: "No series yet")
            } else if filtered.isEmpty {
                EmptyStateView(icon: "questionmark.circle", title: "No series match")
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(filtered.enumerated()), id: \.element.id) { index, entry in
                            NavigationLink(value: Destination.series(id: entry.id)) {
                                row(entry, isFirst: index == 0)
                            }
                            .buttonStyle(PressableStyle())
                        }
                        RecordRule()
                    }
                    .screenPadding()
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Series")
        .searchable(text: $query, prompt: "Filter series")
        .task {
            for await entries in LibraryService.series().values() {
                series = entries
                isLoading = false
            }
            isLoading = false
        }
    }

    /// The count leads in mono, the way a volume number would on a spine.
    private func row(_ entry: SeriesSummary, isFirst: Bool) -> some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                Text("\(entry.bookCount)")
                    .font(.monoUI(13, weight: .medium))
                    .foregroundStyle(palette.accentColor)
                    .frame(width: 26, alignment: .leading)

                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.name)
                        .font(.ui(15, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(1)
                    if let author = entry.primaryAuthor {
                        Text(author)
                            .font(.ui(11.5))
                            .foregroundStyle(palette.ink3Color)
                            .lineLimit(1)
                    }
                }

                Spacer(minLength: 0)

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.ink3Color.opacity(0.7))
            }
            .padding(.vertical, 11)
            .contentShape(Rectangle())
        }
    }
}

struct SeriesDetailView: View {
    let id: Int64

    init(id: Int64) {
        self.id = id
    }

    @Environment(\.palette) private var palette
    @State private var detail: SeriesDetail?
    @State private var isLoading = true

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else if let detail {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(detail.books.enumerated()), id: \.element.id) { index, book in
                            NavigationLink(value: Destination.book(uuid: book.uuid)) {
                                bookRow(book, isFirst: index == 0)
                            }
                            .buttonStyle(PressableStyle())
                        }
                        RecordRule()
                    }
                    .screenPadding()
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
            }
        }
        .background(ScreenBackground())
        .navigationTitle(detail?.name ?? "Series")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            for await value in LibraryService.series(id: id).values() {
                detail = value
                isLoading = false
            }
            isLoading = false
        }
    }

    /// The volume number keeps its column whether or not a book carries one, so
    /// the covers stay on one axis down the page.
    private func bookRow(_ book: Book, isFirst: Bool) -> some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                Text(book.seriesIndex ?? "—")
                    .font(.monoUI(13, weight: .medium))
                    .foregroundStyle(
                        book.seriesIndex == nil ? palette.ink3Color : palette.accentColor
                    )
                    .frame(width: 24, alignment: .leading)

                BookCover(
                    identity: CoverIdentity(
                        uuid: book.uuid,
                        title: book.displayTitle,
                        author: book.creators.first?.name,
                        accent: book.accent,
                        hasCover: book.coverURL != nil
                    ),
                    size: .sm
                )
                .frame(width: 38)
                .coverShadow(0.6)

                VStack(alignment: .leading, spacing: 2) {
                    Text(book.displayTitle)
                        .font(.ui(14.5, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                    Text(book.authorDisplay)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.ink3Color.opacity(0.7))
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
    }
}

// MARK: - Tags

struct TagCloudView: View {
    @Environment(\.palette) private var palette
    @State private var tags: [TagWeight] = []
    @State private var isLoading = true

    /// Weight the type scale by count so the cloud reads as a cloud, with a
    /// floor and ceiling so nothing becomes unreadable or absurd.
    ///
    /// The ramp is square-rooted and the range kept narrow on purpose. On a
    /// library where the commonest tag has four books and the rest have one, a
    /// linear ramp across a 15pt range made a difference of three books look
    /// like a difference in kind — three headlines over a field of small print.
    private func fontSize(for tag: TagWeight) -> CGFloat {
        let counts = tags.map(\.count)
        guard let low = counts.min(), let high = counts.max(), high > low else { return 16 }
        let t = Double(tag.count - low) / Double(high - low)
        return 13.5 + CGFloat(t.squareRoot()) * 9
    }

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else if tags.isEmpty {
                EmptyStateView(icon: "tag", title: "No tags yet")
            } else {
                ScrollView {
                    FlowLayout(spacing: 8, lineSpacing: 10) {
                        ForEach(tags) { tag in
                            NavigationLink(value: Destination.tag(name: tag.name)) {
                                HStack(spacing: 5) {
                                    Text(tag.name)
                                        .font(.ui(fontSize(for: tag), weight: .medium))
                                    Text("\(tag.count)")
                                        .font(.monoUI(max(9, fontSize(for: tag) * 0.6)))
                                        .foregroundStyle(palette.ink3Color)
                                }
                                .foregroundStyle(palette.ink1Color)
                                .padding(.horizontal, 11)
                                .padding(.vertical, 7)
                                .background(Capsule().fill(palette.bg1Color))
                                .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
                            }
                            .buttonStyle(PressableStyle())
                        }
                    }
                    .screenPadding()
                    .padding(.vertical, Spacing.lg)
                }
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Tags")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            for await weights in LibraryService.tags().values() {
                tags = weights.sorted { $0.count > $1.count }
                isLoading = false
            }
            isLoading = false
        }
    }
}
