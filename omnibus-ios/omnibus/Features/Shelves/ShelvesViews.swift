//  ShelvesViews.swift
//  Shelves index, shelf detail, the "add to shelf" picker, and shelf creation.

import SwiftUI

struct ShelvesView: View {
    @Environment(\.palette) private var palette
    @State private var previews: [ShelfPreview] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var showCreate = false

    private let columns = [GridItem(.adaptive(minimum: 112, maximum: 168), spacing: 16)]

    var body: some View {
        Group {
            if isLoading && previews.isEmpty {
                LoadingView()
            } else if let error, previews.isEmpty {
                ErrorStateView(message: error) { Task { await load() } }
            } else if previews.isEmpty {
                EmptyStateView(
                    icon: "square.stack",
                    title: "No shelves yet",
                    message: "Shelves group books by a rule you set, or by hand.",
                    actionTitle: "Create a shelf",
                    action: { showCreate = true }
                )
            } else {
                ScrollView {
                    LazyVGrid(columns: columns, spacing: 26) {
                        ForEach(Array(previews.enumerated()), id: \.element.id) { index, preview in
                            NavigationLink(value: Destination.shelf(id: preview.shelf.id)) {
                                ShelfCard(preview: preview)
                            }
                            .buttonStyle(BookPressStyle())
                            .cascadeIn(index: index)
                            .contextMenu {
                                if !preview.shelf.kind.isSystem {
                                    Button(role: .destructive) {
                                        Task {
                                            try? await UserDataService.deleteShelf(
                                                id: preview.shelf.id
                                            )
                                            await load()
                                        }
                                    } label: {
                                        Label("Delete shelf", systemImage: "trash")
                                    }
                                }
                            }
                        }
                    }
                    .screenPadding()
                    .padding(.vertical, Spacing.lg)
                }
                .scrollIndicators(.hidden)
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Shelves")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { showCreate = true } label: { Image(systemName: "plus") }
            }
        }
        .refreshable { await load(force: true) }
        .sheet(isPresented: $showCreate) {
            CreateShelfSheet { Task { await load(force: true) } }
        }
        .task { await load() }
    }

    private func load(force: Bool = false) async {
        if force { await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews) }
        do {
            previews = try await UserDataService.shelfPreviews()
            error = nil
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        isLoading = false
    }
}

struct ShelfDetailView: View {
    let id: Int64

    init(id: Int64) {
        self.id = id
    }

    @Environment(\.palette) private var palette
    @State private var shelf: Shelf?
    @State private var books: [Book] = []
    @State private var isLoading = true
    @State private var showAddBooks = false
    @State private var scrollY: CGFloat = 0

    private let columns = [GridItem(.adaptive(minimum: 112, maximum: 168), spacing: 16)]

    /// Only a manual shelf can be filled by hand — a smart shelf's membership
    /// is whatever its rules match.
    private var canAddBooks: Bool { shelf?.kind == .manual }

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 30) {
                        header

                        if books.isEmpty {
                            emptyState
                        } else {
                            LazyVGrid(columns: columns, spacing: 26) {
                                ForEach(Array(books.enumerated()), id: \.element.id) { index, book in
                                    NavigationLink(value: Destination.book(uuid: book.uuid)) {
                                        BookGridCell(book: book)
                                    }
                                    .buttonStyle(BookPressStyle())
                                    .cascadeIn(index: index)
                                    .contextMenu {
                                        if shelf?.kind == .manual {
                                            Button(role: .destructive) {
                                                Task {
                                                    await UserDataService.removeBook(
                                                        shelfId: id, uuid: book.uuid
                                                    )
                                                    await load()
                                                }
                                            } label: {
                                                Label("Remove from shelf", systemImage: "minus.circle")
                                            }
                                        }
                                    }
                                }
                            }
                            .screenPadding()
                        }
                    }
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
                .trackingScrollOffset($scrollY)
            }
        }
        .background(ScreenBackground())
        // The header states the shelf's name at full size; the bar picks it up
        // only once that has scrolled away.
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            FadingBarTitle(title: shelf?.name ?? "", scrollY: scrollY, appearsAfter: 76)
        }
        .toolbar {
            if canAddBooks {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        Haptics.tap()
                        showAddBooks = true
                    } label: {
                        Image(systemName: "plus")
                    }
                    .accessibilityLabel("Add books to this shelf")
                }
            }
        }
        .sheet(isPresented: $showAddBooks) {
            if let shelf {
                AddBooksToShelfSheet(
                    shelfId: shelf.id,
                    shelfName: shelf.name,
                    existing: Set(books.map(\.uuid))
                ) {
                    Task { await load(force: true) }
                }
            }
        }
        .refreshable { await load(force: true) }
        .task { await load() }
    }

    /// A shelf you can fill asks to be filled; one driven by rules explains
    /// itself instead of offering an action that wouldn't apply.
    @ViewBuilder
    private var emptyState: some View {
        if canAddBooks {
            Button {
                Haptics.tap()
                showAddBooks = true
            } label: {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Add the first book")
                        .font(.display(17))
                        .foregroundStyle(palette.ink1Color)
                    Text("Search your library and pick as many as you like.")
                        .font(.ui(13))
                        .foregroundStyle(palette.ink3Color)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(Spacing.lg)
                .background(
                    RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                        .strokeBorder(
                            palette.line2.color,
                            style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                        )
                )
            }
            .buttonStyle(.plain)
            .screenPadding()
        } else {
            EmptyStateView(
                icon: "square.stack",
                title: "Nothing here yet",
                message: "No books match this shelf's conditions."
            )
        }
    }

    /// Mosaic beside the shelf's identity, so a smart shelf's conditions are
    /// visible without opening an editor.
    @ViewBuilder
    private var header: some View {
        if let shelf {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                HStack(alignment: .top, spacing: Spacing.lg) {
                    ShelfMosaic(
                        preview: ShelfPreview(shelf: summary(shelf), covers: Array(books.prefix(4)))
                    )
                    .frame(width: 104)
                    .coverShadow(1.2)

                    VStack(alignment: .leading, spacing: 7) {
                        // Same accent lead-in the masthead and book jacket use.
                        Rectangle()
                            .fill(palette.accentColor)
                            .frame(width: 24, height: 1.5)

                        Text(shelf.name)
                            .font(.display(27))
                            .foregroundStyle(palette.ink0Color)
                            .fixedSize(horizontal: false, vertical: true)

                        Text(metaLine(shelf))
                            .font(.monoUI(10, weight: .medium))
                            .tracking(0.7)
                            .textCase(.uppercase)
                            .foregroundStyle(palette.ink3Color)

                        if let description = shelf.description?.nilIfBlank {
                            Text(description)
                                .font(.display(15))
                                .foregroundStyle(palette.ink2Color)
                                .padding(.top, 2)
                        }
                    }

                    Spacer(minLength: 0)
                }

                if shelf.kind == .smart, !shelf.rules.isEmpty {
                    ruleSummary(shelf)
                }
            }
            .screenPadding()
            .padding(.top, Spacing.sm)
        }
    }

    private func ruleSummary(_ shelf: Shelf) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(shelf.matchMode == .any ? "Matches any of" : "Matches all of")
                .font(.monoUI(10, weight: .medium))
                .tracking(0.7)
                .textCase(.uppercase)
                .foregroundStyle(palette.ink3Color)

            FlowLayout(spacing: 5, lineSpacing: 5) {
                ForEach(Array(shelf.rules.enumerated()), id: \.offset) { _, rule in
                    Chip(label: "\(rule.field.label) \(rule.op.label) \(rule.value)")
                }
            }
        }
    }

    private func metaLine(_ shelf: Shelf) -> String {
        var parts = ["\(shelf.bookCount) book\(shelf.bookCount == 1 ? "" : "s")"]
        parts.append(shelf.kind == .smart ? "Smart" : "Manual")
        if shelf.visibility == .public { parts.append("Public") }
        return parts.joined(separator: " · ")
    }

    /// The mosaic takes a summary; detail carries the same fields under a
    /// different type.
    private func summary(_ shelf: Shelf) -> ShelfSummary {
        ShelfSummary(
            id: shelf.id,
            ownerUserId: shelf.ownerUserId,
            ownerUsername: shelf.ownerUsername,
            kind: shelf.kind,
            name: shelf.name,
            visibility: shelf.visibility,
            accent: shelf.accent,
            bookCount: shelf.bookCount
        )
    }

    private func load(force: Bool = false) async {
        if force {
            await OfflineStore.shared.cacheDelete(CacheKey.shelf(id))
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPage(id))
        }
        shelf = try? await UserDataService.shelf(id: id)
        books = (try? await UserDataService.shelfBooks(id: id)) ?? []
        isLoading = false
    }
}

// MARK: - Add to shelf

/// Toggle a book's shelves.
///
/// Was a one-shot "pick a shelf, we add and dismiss" list, which gave no way to
/// see what a book was already on and no way to take it off. Now every manual
/// shelf is a row with its mosaic and a live checkmark, and smart shelves are
/// listed read-only so their absence isn't mistaken for a bug.
struct ShelfPickerSheet: View {
    let book: Book

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var previews: [ShelfPreview] = []
    @State private var member: Set<Int64> = []
    @State private var busy: Set<Int64> = []
    @State private var isLoading = true
    @State private var showCreate = false

    private var manual: [ShelfPreview] { previews.filter { $0.shelf.kind == .manual } }
    private var derived: [ShelfPreview] { previews.filter { $0.shelf.kind != .manual } }

    var body: some View {
        NavigationStack {
            Group {
                if isLoading {
                    LoadingView()
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 28) {
                            VStack(alignment: .leading, spacing: Spacing.sm) {
                                if !manual.isEmpty { SectionLabel("Your shelves") }

                                VStack(spacing: 0) {
                                    ForEach(Array(manual.enumerated()), id: \.element.id) { index, preview in
                                        row(preview, isFirst: index == 0)
                                    }
                                    newShelfRow(isFirst: manual.isEmpty)
                                    RecordRule()
                                }
                            }

                            if !derived.isEmpty {
                                VStack(alignment: .leading, spacing: Spacing.sm) {
                                    SectionLabel("Automatic")

                                    Text("Membership follows each shelf's own rules.")
                                        .font(.ui(12.5))
                                        .foregroundStyle(palette.ink3Color)

                                    VStack(spacing: 0) {
                                        ForEach(Array(derived.enumerated()), id: \.element.id) { index, preview in
                                            row(preview, readOnly: true, isFirst: index == 0)
                                        }
                                        RecordRule()
                                    }
                                }
                            }
                        }
                        .screenPadding()
                        .padding(.vertical, Spacing.lg)
                    }
                    .scrollIndicators(.hidden)
                }
            }
            .background(ScreenBackground())
            .navigationTitle("Add to shelf")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .sheet(isPresented: $showCreate) {
                CreateShelfSheet { Task { await load() } }
            }
        }
        .presentationDetents([.medium, .large])
        .task { await load() }
    }

    private func row(
        _ preview: ShelfPreview, readOnly: Bool = false, isFirst: Bool = false
    ) -> some View {
        let shelf = preview.shelf
        let isOn = member.contains(shelf.id)

        return VStack(spacing: 0) {
            if !isFirst {
                Rectangle()
                    .fill(palette.line2.color)
                    .frame(height: 0.5)
            }

            Button {
                guard !readOnly else { return }
                Task { await toggle(shelf, currentlyOn: isOn) }
            } label: {
                HStack(spacing: Spacing.md) {
                    ShelfMosaic(preview: preview, cornerRadius: 4)
                        .frame(width: 34)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(shelf.name)
                            .font(.ui(15, weight: .medium))
                            .foregroundStyle(palette.ink0Color)
                        Text("\(shelf.bookCount) book\(shelf.bookCount == 1 ? "" : "s")")
                            .font(.ui(11.5))
                            .foregroundStyle(palette.ink3Color)
                    }

                    Spacer(minLength: 0)

                    if busy.contains(shelf.id) {
                        ProgressView()
                    } else if readOnly {
                        Image(systemName: shelf.kind.glyph)
                            .font(.system(size: 13))
                            .foregroundStyle(palette.ink3Color)
                    } else {
                        Image(systemName: isOn ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 21))
                            .foregroundStyle(isOn ? palette.accentColor : palette.ink3Color)
                            .contentTransition(.symbolEffect(.replace))
                    }
                }
                .padding(.vertical, 11)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(readOnly)
        }
    }

    private func newShelfRow(isFirst: Bool) -> some View {
        VStack(spacing: 0) {
            if !isFirst {
                Rectangle()
                    .fill(palette.line2.color)
                    .frame(height: 0.5)
            }

            Button {
                Haptics.tap()
                showCreate = true
            } label: {
                HStack(spacing: Spacing.md) {
                    // Same 2:3 box as a shelf mosaic, so the row's leading
                    // gutter stays one column rather than stepping in.
                    Image(systemName: "plus")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.accentColor)
                        .frame(width: 34, height: 51)
                        .background(
                            RoundedRectangle(cornerRadius: 4, style: .continuous)
                                .strokeBorder(
                                    palette.line2.color,
                                    style: StrokeStyle(lineWidth: 1, dash: [3, 3])
                                )
                        )

                    Text("New shelf")
                        .font(.ui(15, weight: .medium))
                        .foregroundStyle(palette.accentColor)

                    Spacer(minLength: 0)
                }
                .padding(.vertical, 11)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    private func toggle(_ shelf: ShelfSummary, currentlyOn: Bool) async {
        busy.insert(shelf.id)
        defer { busy.remove(shelf.id) }

        if currentlyOn {
            await UserDataService.removeBook(shelfId: shelf.id, uuid: book.uuid)
            member.remove(shelf.id)
        } else {
            await UserDataService.addBooks(shelfId: shelf.id, uuids: [book.uuid])
            member.insert(shelf.id)
        }
        Haptics.select()
        await load(quiet: true)
    }

    private func load(quiet: Bool = false) async {
        if !quiet { isLoading = true }
        await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews)
        previews = (try? await UserDataService.shelfPreviews()) ?? []
        member = await UserDataService.shelvesContaining(uuid: book.uuid)
        isLoading = false
    }
}
