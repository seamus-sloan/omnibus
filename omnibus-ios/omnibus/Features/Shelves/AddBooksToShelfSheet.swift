//  AddBooksToShelfSheet.swift
//  Pick books to put on a shelf, from inside the shelf.
//
//  Filling a shelf previously meant leaving it, opening each book's detail
//  screen, and adding them one at a time through the shelf picker — so building
//  a ten-book shelf cost ten round trips. This is the same operation from the
//  other direction: browse or search the library, select as many as you like,
//  add them in one call.

import SwiftUI

struct AddBooksToShelfSheet: View {
    let shelfId: Int64
    let shelfName: String
    /// Books already on the shelf, shown as such rather than offered again.
    let existing: Set<String>
    var onAdded: () -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var query = ""
    @State private var books: [Book] = []
    @State private var selected: Set<String> = []
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var searchTask: Task<Void, Never>?

    var body: some View {
        NavigationStack {
            Group {
                if isLoading && books.isEmpty {
                    LoadingView()
                } else if books.isEmpty {
                    EmptyStateView(
                        icon: "magnifyingglass",
                        title: "No matches",
                        message: "Nothing in the library matches “\(query)”."
                    )
                } else {
                    list
                }
            }
            .background(ScreenBackground())
            .navigationTitle(shelfName)
            .navigationBarTitleDisplayMode(.inline)
            .searchable(
                text: $query,
                placement: .navigationBarDrawer(displayMode: .always),
                prompt: "Search your library"
            )
            .onChange(of: query) { _, value in schedule(value) }
            .toolbar { toolbar }
        }
        .presentationDetents([.large])
        .task { await loadLibrary() }
    }

    private var list: some View {
        List {
            ForEach(books) { book in
                row(book)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .scrollDismissesKeyboard(.interactively)
    }

    private func row(_ book: Book) -> some View {
        let alreadyOn = existing.contains(book.uuid)
        let isPicked = selected.contains(book.uuid)

        return Button {
            guard !alreadyOn else { return }
            Haptics.select()
            if isPicked {
                selected.remove(book.uuid)
            } else {
                selected.insert(book.uuid)
            }
        } label: {
            HStack(spacing: Spacing.md) {
                BookCover(identity: CoverIdentity(book), size: .sm, cornerRadius: 4)
                    .frame(width: 38)

                VStack(alignment: .leading, spacing: 2) {
                    Text(book.displayTitle)
                        .font(.ui(15, weight: .medium))
                        .foregroundStyle(alreadyOn ? palette.ink3Color : palette.ink0Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)

                    Text(alreadyOn ? "Already on this shelf" : book.authorDisplay)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)

                marker(alreadyOn: alreadyOn, isPicked: isPicked)
            }
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(alreadyOn)
        .listRowBackground(Color.clear)
        .listRowSeparatorTint(palette.line2.color)
    }

    @ViewBuilder
    private func marker(alreadyOn: Bool, isPicked: Bool) -> some View {
        if alreadyOn {
            Image(systemName: "checkmark")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(palette.ink3Color)
        } else {
            Image(systemName: isPicked ? "checkmark.circle.fill" : "circle")
                .font(.system(size: 21))
                .foregroundStyle(isPicked ? palette.accentColor : palette.ink3Color)
                .contentTransition(.symbolEffect(.replace))
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) {
            Button("Cancel") { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
            Button(selected.isEmpty ? "Add" : "Add \(selected.count)") {
                Task { await add() }
            }
            .disabled(selected.isEmpty || isSaving)
        }
    }

    // MARK: - Data

    /// Debounced so typing doesn't fire a request per keystroke.
    private func schedule(_ value: String) {
        searchTask?.cancel()
        searchTask = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            let trimmed = value.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty {
                await loadLibrary()
            } else {
                await search(trimmed)
            }
        }
    }

    private func loadLibrary() async {
        isLoading = true
        defer { isLoading = false }
        for await page in LibraryService.page(
            sort: .newestAdded, direction: .desc, filter: .none, cursor: nil
        ).values() {
            books = page.books
        }
    }

    private func search(_ text: String) async {
        isLoading = true
        defer { isLoading = false }
        for await hits in LibraryService.searchFull(query: text) {
            books = hits
        }
    }

    private func add() async {
        isSaving = true
        defer { isSaving = false }
        await UserDataService.addBooks(shelfId: shelfId, uuids: Array(selected))
        Haptics.success()
        onAdded()
        dismiss()
    }
}
