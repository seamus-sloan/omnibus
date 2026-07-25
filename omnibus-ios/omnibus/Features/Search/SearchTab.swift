//  SearchTab.swift
//  The Search tab: a field, and — while it's empty — the ways into the library
//  that aren't "a book I can name".
//
//  Authors, series, and tags used to be pills on the landing screen, which is
//  the wrong place: they're all answers to "help me find something", which is
//  what this tab is for. Moving them here is what let the landing become mostly
//  books.

import SwiftUI

struct SearchTab: View {
    @Environment(\.palette) private var palette
    @Environment(TabReselect.self) private var reselect
    @State private var path = NavigationPath()

    var body: some View {
        NavigationStack(path: $path) {
            SearchView()
                .withDestinations()
        }
        // Tapping Search while already in Search returns to the plain search
        // page — unwinding any pushed authors/series/tags screens.
        .onChange(of: reselect.token) { _, _ in
            guard reselect.tab == .search, !path.isEmpty else { return }
            withAnimation(Motion.glide) { path.removeLast(path.count) }
        }
    }
}

/// Shown under the search field before anything is typed.
///
/// Four hairline rows left most of the screen empty and gave no sense of what
/// was behind them. As tiles the four ways in are one glance rather than four
/// reads, and the space they now take is space the screen had going spare.
struct BrowseDirectory: View {
    @Environment(\.palette) private var palette

    @State private var finished: [FinishedBook] = []

    private let columns = [
        GridItem(.flexible(), spacing: Spacing.md),
        GridItem(.flexible(), spacing: Spacing.md),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 30) {
            VStack(alignment: .leading, spacing: Spacing.md) {
                SectionLabel("Browse")

                LazyVGrid(columns: columns, spacing: Spacing.md) {
                    tile("Authors", icon: "person.2", note: "Who wrote it", to: .authorsIndex)
                    tile("Series", icon: "books.vertical", note: "In order", to: .seriesIndex)
                    tile("Shelves", icon: "square.stack", note: "Your groupings", to: .shelves)
                    tile("Tags", icon: "tag", note: "By subject", to: .tags)
                }
            }
            .screenPadding()

            if !finished.isEmpty {
                recentlyFinished
            }
        }
        .task { await loadFinished() }
    }

    private func tile(
        _ label: String, icon: String, note: String, to destination: Destination
    ) -> some View {
        NavigationLink(value: destination) {
            VStack(alignment: .leading, spacing: 0) {
                Image(systemName: icon)
                    .font(.system(size: 21, weight: .light))
                    .foregroundStyle(palette.accentColor)
                    .frame(height: 26)

                Spacer(minLength: Spacing.sm)

                Text(label)
                    .font(.display(19))
                    .foregroundStyle(palette.ink0Color)

                Text(note)
                    .font(.ui(11.5))
                    .foregroundStyle(palette.ink3Color)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .frame(height: 102)
            .padding(14)
            .background(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .fill(palette.bg1Color)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 0.5)
            )
        }
        .buttonStyle(PressableStyle())
    }

    /// The one thing worth showing off on a screen that would otherwise be an
    /// empty field: what you've actually finished.
    ///
    /// Covers alone were unlabelled — a stranger's shelf you couldn't read. The
    /// caption carries the title, and the rating or the date sits under it the
    /// way it does everywhere else a book is listed.
    private var recentlyFinished: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Recently finished")
                .screenPadding()

            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 14) {
                    ForEach(finished) { book in
                        NavigationLink(value: Destination.book(uuid: book.bookUUID)) {
                            finishedCard(book)
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

    private func finishedCard(_ book: FinishedBook) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            BookCover(
                identity: CoverIdentity(
                    uuid: book.bookUUID,
                    title: book.title,
                    author: book.author,
                    hasCover: book.coverURL != nil
                ),
                size: .md
            )
            .coverShadow()

            VStack(alignment: .leading, spacing: 3) {
                Text(book.title)
                    .font(.ui(12.5, weight: .medium))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)

                if let rating = book.rating {
                    StarRating(stars: rating, size: 10)
                } else {
                    Text(Format.relative(unix: book.finishedAt))
                        .font(.ui(10.5))
                        .foregroundStyle(palette.ink3Color)
                }
            }
        }
        .frame(width: 100)
    }

    private func loadFinished() async {
        guard finished.isEmpty else { return }
        let summary = try? await UserDataService.stats(range: .allTime)
        finished = Array((summary?.finishedBooks ?? []).prefix(10))
    }
}
