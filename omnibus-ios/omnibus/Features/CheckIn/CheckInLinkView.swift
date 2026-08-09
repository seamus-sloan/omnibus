//  CheckInLinkView.swift
//  Pick the library book a scanned copy belongs to, when no rung of the
//  matching ladder could reach it — a print barcode the ebook never published,
//  under a provider title that shares nothing with the shelf copy.
//
//  Search only: the pick goes back out through `onPick`, and the flow reuses
//  the ordinary in-library confirm to file the copy, so there is one write
//  path and one success screen rather than two.

import SwiftUI

struct CheckInLinkView: View {
    /// The ISBN the copy will be filed under, carried onto the picked book.
    let isbn: String?
    var onPick: (ScanBook) -> Void
    var onBack: () -> Void

    @Environment(\.palette) private var palette

    @State private var query = ""
    @State private var isSearching = false
    /// `nil` until the first search lands, so the no-matches line can't show
    /// before anyone has searched.
    @State private var results: PaletteResults?
    @State private var searchTask: Task<Void, Never>?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                header
                searchField
                resultsList
                Button("Back") { onBack() }
                    .font(.ui(14))
                    .foregroundStyle(palette.accentColor)
                    .frame(maxWidth: .infinity)
                    .padding(.top, Spacing.sm)
            }
            .screenPadding()
            .padding(.vertical, Spacing.lg)
        }
        .onDisappear { searchTask?.cancel() }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Which book is this?")
                .font(.display(22))
                .foregroundStyle(palette.ink0Color)
            Text(
                "Find it in your library and we'll file this copy against it — no new book, no duplicate to merge later."
            )
            .font(.ui(14))
            .foregroundStyle(palette.ink2Color)
        }
    }

    private var searchField: some View {
        HStack {
            TextField("Title or author", text: $query)
                .textFieldStyle(OmnibusFieldStyle())
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .submitLabel(.search)
                .onChange(of: query) { _, value in schedule(value) }
            if isSearching {
                ProgressView()
            }
        }
        .accessibilityIdentifier("check-in-link-query")
    }

    @ViewBuilder
    private var resultsList: some View {
        if let results {
            if results.books.isEmpty {
                Text(
                    "No book in your library matches that. Try the author's name, or a distinctive word from the title."
                )
                .font(.ui(13))
                .foregroundStyle(palette.ink2Color)
            } else {
                VStack(spacing: Spacing.sm) {
                    ForEach(results.books) { hit in
                        Button {
                            onPick(CheckInFlow.linkTarget(from: hit, isbn: isbn))
                        } label: {
                            resultRow(hit)
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("check-in-link-result")
                    }
                }
                if let note = CheckInFlow.linkTruncationNote(
                    shown: results.books.count, total: results.bookTotal
                ) {
                    Text(note)
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink3Color)
                }
            }
        }
    }

    private func resultRow(_ hit: PaletteBookHit) -> some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            BookCover(
                identity: CoverIdentity(
                    uuid: hit.uuid, title: hit.title, author: hit.authorDisplay,
                    accent: hit.accent, hasCover: hit.coverURL != nil
                ),
                size: .sm
            )
            .frame(width: 44)

            VStack(alignment: .leading, spacing: 3) {
                Text(hit.title)
                    .font(.ui(14, weight: .medium))
                    .foregroundStyle(palette.ink0Color)
                    .multilineTextAlignment(.leading)
                Text(hit.authorDisplay)
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink2Color)
                if let year = hit.year {
                    Text(year)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                }
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(palette.ink3Color)
                .padding(.top, 4)
        }
        .padding(Spacing.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                .fill(palette.bg1Color)
        )
    }

    /// Restart the search on every keystroke, mirroring `SearchView`: the
    /// service answers from the local mirror straight away and from the server
    /// after its own debounce, so cancelling here cancels a server read the
    /// reader has already typed past.
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
