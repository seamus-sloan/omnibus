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
                    message: "A shelf gathers books by hand, or fills itself from a rule you set.",
                    kicker: "Your shelves",
                    fanned: true,
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
                                            await UserDataService.deleteShelf(id: preview.shelf.id)
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
        .refreshTask { await load(force: true) }
        .sheet(isPresented: $showCreate) {
            CreateShelfSheet { Task { await load(force: true) } }
        }
        .task { await load() }
    }

    private func load(force: Bool = false) async {
        // No cache-clear on the way in, even for a pull. `Cache.live` refetches
        // on every read regardless — the delete only forced it to re-*yield* an
        // identical answer — and offline it threw away the only copy of the
        // shelves there was, so pulling to refresh with no signal emptied the
        // screen.
        do {
            for try await read in UserDataService.shelfPreviews() {
                previews = read.value
                error = nil
                isLoading = false
            }
        } catch {
            if previews.isEmpty {
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
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
    /// The shelf's identity as the cached index knows it, for when the detail
    /// read hasn't landed.
    ///
    /// `UserDataService.shelf(id:)` is the only read carrying rules and a
    /// description, and on a cold cache it is network-only — so offline this
    /// page lost its name, its mosaic and its *kind* at once, and with the kind
    /// gone it told a manual shelf that nothing matched its conditions. The
    /// shelves index is already in the replica and carries every field the
    /// header needs.
    @State private var indexed: ShelfSummary?
    @State private var books: [Book] = []
    @State private var isLoading = true
    @State private var showAddBooks = false
    @State private var scrollY: CGFloat = 0

    // `.top` keeps `BookGridCell`s aligned by their cover art when captions
    // in the same row wrap to different line counts — see LibraryView.swift.
    private let columns = [GridItem(.adaptive(minimum: 112, maximum: 168), spacing: 16, alignment: .top)]

    /// Whatever is known about this shelf, detail read first.
    private var identity: ShelfSummary? { shelf.map(Self.summary) ?? indexed }

    /// Only a manual shelf can be filled by hand — a smart shelf's membership
    /// is whatever its rules match, and the wishlist's is what you check in.
    private var canAddBooks: Bool { identity?.kind == .manual }

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 30) {
                        header

                        if books.isEmpty {
                            // The index knows the count; only the member page
                            // carries the books. Offline with the page uncached
                            // we know there are some and can't show them, which
                            // is not the same as the shelf being empty — and
                            // saying "nothing on this shelf" under a header
                            // reading "1 book" contradicts itself.
                            if let identity, identity.bookCount > 0 {
                                unreachableState(count: identity.bookCount)
                            } else {
                                emptyState
                            }
                        } else {
                            LazyVGrid(columns: columns, spacing: 26) {
                                ForEach(Array(books.enumerated()), id: \.element.id) { index, book in
                                    NavigationLink(value: Destination.book(uuid: book.uuid)) {
                                        BookGridCell(book: book)
                                    }
                                    .buttonStyle(BookPressStyle())
                                    .cascadeIn(index: index)
                                    .bookContextMenu(book, onEdited: { Task { await load() } }) {
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
            FadingBarTitle(title: identity?.name ?? "", scrollY: scrollY, appearsAfter: 76)
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
            if let identity {
                AddBooksToShelfSheet(
                    shelfId: identity.id,
                    shelfName: identity.name,
                    existing: Set(books.map(\.uuid))
                ) {
                    Task { await load(force: true) }
                }
            }
        }
        .refreshTask { await load(force: true) }
        .task { await load() }
    }

    /// A shelf you can fill asks to be filled; one driven by rules explains
    /// itself instead of offering an action that wouldn't apply.
    private var emptyState: some View {
        let copy = ShelfEmptyCopy.forKind(identity?.kind)

        return EmptyStateView(
            icon: (identity?.kind ?? .manual).glyph,
            title: copy.title,
            message: copy.message,
            kicker: copy.kicker,
            fanned: true,
            actionTitle: copy.actionTitle,
            action: copy.actionTitle == nil ? nil : { showAddBooks = true }
        )
        // Inside the page's scroll view this sizes to its content, so give it
        // enough of the pane to sit in rather than letting it hug the header.
        .frame(minHeight: 380)
    }

    /// The shelf has books the device hasn't got a copy of.
    private func unreachableState(count: Int64) -> some View {
        EmptyStateView(
            icon: "arrow.trianglehead.2.clockwise.rotate.90",
            title: "Not here yet",
            message: ShelfEmptyCopy.unreachable(count: count),
            kicker: "Off the network",
            fanned: true
        )
        .frame(minHeight: 380)
    }

    /// Mosaic beside the shelf's identity, so a smart shelf's conditions are
    /// visible without opening an editor.
    @ViewBuilder
    private var header: some View {
        if let identity {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                HStack(alignment: .top, spacing: Spacing.lg) {
                    ShelfMosaic(
                        preview: ShelfPreview(shelf: identity, covers: Array(books.prefix(4)))
                    )
                    .frame(width: 104)
                    .coverShadow(1.2)

                    VStack(alignment: .leading, spacing: 7) {
                        // Same accent lead-in the masthead and book jacket use.
                        Rectangle()
                            .fill(palette.accentColor)
                            .frame(width: 24, height: 1.5)

                        Text(identity.name)
                            .font(.display(30))
                            .foregroundStyle(palette.ink0Color)
                            .fixedSize(horizontal: false, vertical: true)

                        Text(Self.metaLine(identity))
                            .font(.monoUI(10, weight: .medium))
                            .tracking(0.7)
                            .textCase(.uppercase)
                            .foregroundStyle(palette.ink3Color)

                        if let description = shelf?.description?.nilIfBlank {
                            Text(description)
                                .font(.display(18))
                                .foregroundStyle(palette.ink2Color)
                                .padding(.top, 2)
                        }
                    }

                    Spacer(minLength: 0)
                }

                if let shelf, shelf.kind == .smart, !shelf.rules.isEmpty {
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

    /// Every kind names itself. It used to print "Manual" for anything that
    /// wasn't smart, so the wishlist — which nobody fills by hand — described
    /// itself as the one kind you do.
    static func metaLine(_ shelf: ShelfSummary) -> String {
        var parts = ["\(shelf.bookCount) book\(shelf.bookCount == 1 ? "" : "s")"]
        switch shelf.kind {
        case .smart: parts.append("Smart")
        case .manual: parts.append("Manual")
        case .wishlist: parts.append("Wishlist")
        }
        if shelf.visibility == .public { parts.append("Public") }
        return parts.joined(separator: " · ")
    }

    /// The mosaic takes a summary; detail carries the same fields under a
    /// different type.
    static func summary(_ shelf: Shelf) -> ShelfSummary {
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
        await withTaskGroup(of: Void.self) { group in
            group.addTask { @MainActor in
                // Cheap, local, and only ever a fallback — the detail read
                // wins the moment it lands.
                let previews: [ShelfPreview]? = await Cache.cachedOnly(CacheKey.shelfPreviews)
                self.indexed = previews?.first { $0.shelf.id == self.id }?.shelf
            }
            group.addTask { @MainActor in
                for await value in UserDataService.shelf(id: id).values() {
                    self.shelf = value
                    self.isLoading = false
                }
            }
            group.addTask { @MainActor in
                for await page in UserDataService.shelfBooks(id: id).values() {
                    self.books = page.books
                    self.isLoading = false
                }
            }
        }
        isLoading = false
    }
}

/// What a shelf with nothing on it should say.
///
/// A table rather than a branch in the view: the three kinds are filled by
/// three different mechanisms — by hand, by a rule, by checking a book in —
/// so only one of them has an action to offer, and telling the other two that
/// "no books match this shelf's conditions" describes machinery they don't
/// have. `nil` is the honest fourth case: offline before the detail read has
/// landed, the kind isn't known, and the page must not guess at one.
enum ShelfEmptyCopy {
    struct Copy: Equatable {
        let kicker: String
        let title: String
        let message: String
        /// Non-nil only where the reader can actually fill this shelf here.
        let actionTitle: String?
    }

    static func forKind(_ kind: ShelfKind?) -> Copy {
        switch kind {
        case .manual:
            Copy(
                kicker: "Empty shelf",
                title: "Nothing on this shelf",
                message: "Pick books out of your library and they gather here, in the order you add them.",
                actionTitle: "Add books"
            )
        case .smart:
            Copy(
                kicker: "No matches",
                title: "Nothing matches yet",
                message: "This shelf fills itself from its rules. Nothing in the library meets them right now.",
                actionTitle: nil
            )
        case .wishlist:
            Copy(
                kicker: "Wishlist",
                title: "Nothing on the wishlist",
                message: "Check in a book you don't own yet and it waits here until a copy arrives.",
                actionTitle: nil
            )
        case nil:
            Copy(
                kicker: "Empty shelf",
                title: "Nothing on this shelf",
                message: "No books are on it yet.",
                actionTitle: nil
            )
        }
    }

    /// What to say when the shelf is known to hold books the device hasn't
    /// fetched — offline, with the member page never cached. States the count
    /// so the line agrees with the header rather than contradicting it.
    static func unreachable(count: Int64) -> String {
        count == 1
            ? "This shelf holds one book, but it hasn't reached this device. "
                + "It'll appear the next time you're online."
            : "This shelf holds \(count) books, but they haven't reached this device. "
                + "They'll appear the next time you're online."
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
        // Deliberately not clearing the cache first: the live read refetches
        // anyway, and dropping it offline leaves this sheet with no shelves to
        // show at all.
        for await value in UserDataService.shelfPreviews().values() { previews = value }
        for await ids in UserDataService.shelvesContaining(uuid: book.uuid).values() {
            member = Set(ids)
        }
        isLoading = false
    }
}
