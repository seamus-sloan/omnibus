//  MetadataEditView.swift
//  Metadata override editor.
//
//  Built as an editable colophon rather than a `Form`: the detail screen states
//  a book's printing details as mono small-caps keys over the reading face, and
//  this is the same plate with the values made editable. A stock `Form` also
//  used each field's placeholder as its only label, so a filled row lost its
//  name — "en" with nothing to say it meant Language.

import SwiftUI

struct MetadataEditView: View {
    let uuid: String
    /// Runs after a save or revert lands, before the editor dismisses — the
    /// long-press entry points use it to refresh the grid behind their sheet.
    var onSaved: (() -> Void)?

    init(uuid: String, onSaved: (() -> Void)? = nil) {
        self.uuid = uuid
        self.onSaved = onSaved
    }

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var book: Book?
    @State private var draft = MetadataDraft()
    @State private var loaded = MetadataDraft()
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var error: String?
    /// An author typed into the add field but not yet committed to a chip.
    /// Held here rather than inside the chip field so saving can flush it.
    @State private var pendingAuthor = ""
    /// Same, for the tags field.
    @State private var pendingTag = ""
    /// Same, for the genres field.
    @State private var pendingGenre = ""

    /// Set to `errorAnchor` to scroll the error banner into view, then
    /// cleared by `ScrollViewReader`'s `onChange`. A plain `Bool` wouldn't
    /// re-fire for a second failed save.
    @State private var scrollTarget: String?
    private static let errorAnchor = "metadata-edit-error"

    /// Whether this instance has any provider to search. Offering a search
    /// that could only fail is worse than not offering one, so the trigger
    /// renders nothing until the catalog says otherwise.
    @State private var canFetch = false
    @State private var showFetch = false
    /// What the fetch sheet staged on its way out, so the reader can see that
    /// something happened without hunting the form for accent dots.
    @State private var fetchNote: String?

    /// Autocomplete pools for the chip and series fields, filled best-effort
    /// while the editor is up — an empty pool just means no dropdown.
    @State private var authorPool: [SuggestionItem] = []
    @State private var tagPool: [SuggestionItem] = []
    @State private var genrePool: [SuggestionItem] = []
    @State private var seriesPool: [SuggestionItem] = []

    private var pendingAuthorName: String {
        pendingAuthor.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var pendingTagName: String {
        pendingTag.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var pendingGenreName: String {
        pendingGenre.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var isDirty: Bool {
        draft != loaded || !pendingAuthorName.isEmpty || !pendingTagName.isEmpty
            || !pendingGenreName.isEmpty
    }

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else {
                content
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Edit metadata")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { toolbar }
        .task { await load() }
        .task { await loadSuggestionPools() }
        .task { await checkProviders() }
        .sheet(isPresented: $showFetch) { fetchSheet }
    }

    /// Presented rather than pushed: finding an edition is a detour off the
    /// form, not a step through it, and a sheet is what says "you'll come back
    /// here" on iOS.
    @ViewBuilder
    private var fetchSheet: some View {
        if let book {
            MetadataFetchSheet(
                uuid: uuid,
                identity: CoverIdentity(book),
                draft: $draft,
                loaded: loaded
            ) { note in
                guard let note else { return }
                withAnimation(Motion.settle) { fetchNote = note }
            }
        }
    }

    private var content: some View {
        ScrollViewReader { proxy in
            formScroll
                .onChange(of: scrollTarget) { _, target in
                    guard let target else { return }
                    // `.center`, not `.bottom`: the bottom of the scroll view
                    // is where the tab bar and a still-retracting keyboard
                    // are, and a message parked under either is no message.
                    proxy.scrollTo(target, anchor: .center)
                    scrollTarget = nil
                }
        }
    }

    private var formScroll: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                if let book { header(book) }

                if canFetch, book != nil { fetchButton }
                if let fetchNote { fetchNoteLine(fetchNote) }

                group("Title & authors") {
                    Plate {
                        field("Title", \.title, isFirst: true)
                        ChipListField(
                            label: "Authors",
                            placeholder: "Add an author",
                            values: $draft.authors,
                            entry: $pendingAuthor,
                            isEdited: draft.authors != loaded.authors,
                            suggestions: authorPool
                        )
                    }
                }

                group("Series") {
                    Plate {
                        field("Series", \.series, isFirst: true, suggestions: seriesPool)
                        field("Index", \.seriesIndex, keyboard: .decimalPad)
                    }
                }

                group("Publication") {
                    Plate {
                        field("Publisher", \.publisher, isFirst: true)
                        field("Published", \.published, hint: "YYYY-MM-DD")
                        field("Language", \.language, hint: "e.g. en")
                        field("ISBN-13", \.isbn13, keyboard: .numberPad)
                        // Not `.numberPad`: an ISBN-10's check digit can be X.
                        field("ISBN-10", \.isbn10, hint: "10 characters", keyboard: .asciiCapable)
                        field("Print Pages", \.printPages, hint: "whole number", keyboard: .numberPad)
                    }
                }

                group("Tags") {
                    Plate {
                        ChipListField(
                            label: "Tags",
                            placeholder: "Add a tag",
                            values: $draft.tags,
                            entry: $pendingTag,
                            isEdited: draft.tags != loaded.tags,
                            deduplicates: true,
                            isFirst: true,
                            suggestions: tagPool
                        )
                    }
                }

                group("Genres") {
                    Plate {
                        ChipListField(
                            label: "Genres",
                            placeholder: "Add a genre",
                            values: $draft.genres,
                            entry: $pendingGenre,
                            isEdited: draft.genres != loaded.genres,
                            deduplicates: true,
                            isFirst: true,
                            suggestions: genrePool
                        )
                    }
                }

                group("Description") {
                    Plate {
                        field("Summary", \.description, isFirst: true, multiline: true)
                    }
                }

                if let error {
                    Text(error)
                        .font(.ui(13))
                        .foregroundStyle(palette.badColor)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .id(Self.errorAnchor)
                        .accessibilityIdentifier("metadata-edit-error")
                }

                if book?.hasOverride == true {
                    revertButton
                }
            }
            .screenPadding()
            .padding(.top, Spacing.md)
            .padding(.bottom, 48)
        }
        .scrollIndicators(.hidden)
    }

    // MARK: - Pieces

    /// Which book you're editing — a title in the bar alone isn't enough
    /// anchoring once the fields are full of someone else's punctuation.
    private func header(_ book: Book) -> some View {
        HStack(spacing: Spacing.md) {
            BookCover(identity: CoverIdentity(book), size: .sm, cornerRadius: 4)
                .frame(width: 48)
                .coverShadow(0.6)

            VStack(alignment: .leading, spacing: 3) {
                Text(book.displayTitle)
                    .font(.display(19))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(2)

                Text(book.hasOverride ? "Edited" : "As scanned")
                    .font(.monoUI(10, weight: .medium))
                    .tracking(0.7)
                    .textCase(.uppercase)
                    .foregroundStyle(book.hasOverride ? palette.accentColor : palette.ink3Color)
            }

            Spacer(minLength: 0)
        }
    }

    private func group<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(title)
                .font(.display(20))
                .foregroundStyle(palette.ink0Color)
            content()
        }
    }

    private func field(
        _ label: String,
        _ key: WritableKeyPath<MetadataDraft, String>,
        isFirst: Bool = false,
        hint: String? = nil,
        keyboard: UIKeyboardType = .default,
        multiline: Bool = false,
        suggestions: [SuggestionItem] = []
    ) -> some View {
        PlateField(
            label: label,
            text: Binding(
                get: { draft[keyPath: key] },
                set: { draft[keyPath: key] = $0 }
            ),
            isEdited: draft[keyPath: key] != loaded[keyPath: key],
            isFirst: isFirst,
            hint: hint,
            keyboard: keyboard,
            multiline: multiline,
            suggestions: suggestions
        )
    }

    /// The way into the provider search. Sits above the fields rather than in
    /// the toolbar because it is the fastest route through this whole screen —
    /// a reader fixing a badly-scanned book should find it before they start
    /// typing, not after.
    private var fetchButton: some View {
        Button {
            Haptics.tap()
            fetchNote = nil
            showFetch = true
        } label: {
            HStack(spacing: 11) {
                Image(systemName: "sparkle.magnifyingglass")
                    .font(.system(size: 17, weight: .medium))
                    .foregroundStyle(palette.accentColor)

                VStack(alignment: .leading, spacing: 2) {
                    Text("Fetch metadata")
                        .font(.ui(15, weight: .semibold))
                        .foregroundStyle(palette.ink0Color)
                    Text("Search Open Library, Google Books and more")
                        .font(.ui(12))
                        .foregroundStyle(palette.ink2Color)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)

                Image(systemName: "chevron.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink3Color)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 13)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .fill(palette.bg1Color)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .strokeBorder(palette.accentColor.opacity(0.35), lineWidth: 1)
            )
        }
        .buttonStyle(PressableStyle())
        .accessibilityIdentifier("metadata-fetch-open")
    }

    /// What the sheet staged, stated once. It clears on the next fetch and on
    /// a save, so it can never outlive the edit it describes.
    private func fetchNoteLine(_ note: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 13))
                .foregroundStyle(palette.accentColor)
            Text(note)
                .font(.ui(13))
                .foregroundStyle(palette.ink1Color)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .transition(.opacity.combined(with: .move(edge: .top)))
        .accessibilityIdentifier("metadata-fetch-note")
    }

    private var revertButton: some View {
        Button {
            Haptics.warning()
            Task { await revert() }
        } label: {
            Label("Revert to scanned metadata", systemImage: "arrow.uturn.backward")
                .font(.ui(14, weight: .medium))
                .foregroundStyle(palette.badColor)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 13)
                .background(
                    RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                        .fill(palette.bg1Color)
                )
        }
        .buttonStyle(.plain)
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .confirmationAction) {
            Button("Save") { Task { await save() } }
                // Nothing to save until something actually differs from what
                // was loaded, so the control states that rather than always
                // inviting a no-op write.
                .disabled(isSaving || isLoading || !isDirty)
        }
    }

    // MARK: - Data

    private func load() async {
        book = try? await LibraryService.settledBook(uuid: uuid)
        guard let book else {
            isLoading = false
            return
        }
        loaded = MetadataDraft(book: book)
        draft = loaded
        isLoading = false
    }

    /// Whether to offer the provider search at all. One read, best-effort: a
    /// catalog that can't be fetched leaves the trigger absent, which is the
    /// same answer an instance with no configured source gets.
    private func checkProviders() async {
        let catalog: [ProviderInfo] = (try? await APIClient.shared.get("/api/metadata/providers")) ?? []
        withAnimation(Motion.settle) { canFetch = catalog.contains(where: \.configured) }
    }

    /// Fill the four autocomplete pools, mirroring the web editor's
    /// `use_suggestion_pools`. Each pool streams replica-first (`Cache.live`),
    /// so a cached listing paints suggestions immediately and the server's
    /// answer refines them; a failed read just leaves that pool empty.
    private func loadSuggestionPools() async {
        async let authors: Void = collectAuthorPool()
        async let tags: Void = collectTagPool()
        async let genres: Void = collectGenrePool()
        async let series: Void = collectSeriesPool()
        _ = await (authors, tags, genres, series)
    }

    private func collectAuthorPool() async {
        for await authors in LibraryService.authors().values() {
            authorPool = SuggestionPool.collect(
                authors.map { SuggestionItem(name: $0.name, count: $0.bookCount) }
            )
        }
    }

    private func collectTagPool() async {
        for await tags in LibraryService.tags().values() {
            tagPool = SuggestionPool.collect(
                tags.map { SuggestionItem(name: $0.name, count: $0.count) }
            )
        }
    }

    private func collectGenrePool() async {
        for await genres in LibraryService.genres().values() {
            genrePool = SuggestionPool.collect(
                genres.map { SuggestionItem(name: $0.name, count: $0.count) }
            )
        }
    }

    private func collectSeriesPool() async {
        for await series in LibraryService.series().values() {
            seriesPool = SuggestionPool.collect(
                series.map { SuggestionItem(name: $0.name, count: $0.bookCount) }
            )
        }
    }

    private func save() async {
        error = nil

        // Before the chip flush below, which mutates `draft`: a rejected save
        // has to leave the form exactly as the reader left it, or a bad page
        // count silently consumes the author they were half-way through
        // typing. The web editor orders it the same way, rejecting in
        // `build_on_save`'s first statement.
        do {
            try draft.validate(since: loaded)
        } catch {
            reportSaveFailure(error)
            return
        }

        isSaving = true
        defer { isSaving = false }

        flushPendingChips()

        let body = draft.payload(since: loaded)
        // Nothing to send. Not merely wasteful: an empty body makes the
        // server insert an override row that latches this book to "Edited"
        // permanently — see `MetadataOverridesPayload.isEmpty`.
        guard !body.isEmpty else {
            dismiss()
            return
        }

        do {
            let _: Empty = try await APIClient.shared.post(
                "/api/ebooks/\(uuid)/overrides", body: body
            )
            await OfflineStore.shared.cacheDelete(CacheKey.book(uuid))
            Haptics.success()
            onSaved?()
            dismiss()
        } catch {
            reportSaveFailure(error)
        }
    }

    /// Surface a failed save. The message alone isn't enough: it renders at
    /// the bottom of a scrolling form, well below the fields that cause it,
    /// so without the haptic and the scroll a rejected Save looks to the
    /// reader like a Save that did nothing at all. The keyboard has to go
    /// too — the field being rejected is usually the one still focused, and
    /// its keypad covers exactly the part of the form the message lands in.
    private func reportSaveFailure(_ error: Error) {
        self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        Haptics.warning()
        dismissKeyboard()
        withAnimation(Motion.snap) { scrollTarget = Self.errorAnchor }
    }

    private func dismissKeyboard() {
        UIApplication.shared.sendAction(
            #selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil
        )
    }

    /// A value typed into an add field but never committed to a chip is still
    /// what the user meant to save; dropping it silently is worse than
    /// accepting it.
    private func flushPendingChips() {
        if let chip = ChipEntry.committed(
            from: pendingAuthor, existing: draft.authors, deduplicating: false
        ) {
            draft.authors.append(chip)
        }
        pendingAuthor = ""
        if let chip = ChipEntry.committed(
            from: pendingTag, existing: draft.tags, deduplicating: true
        ) {
            draft.tags.append(chip)
        }
        pendingTag = ""
        if let chip = ChipEntry.committed(
            from: pendingGenre, existing: draft.genres, deduplicating: true
        ) {
            draft.genres.append(chip)
        }
        pendingGenre = ""
    }

    private func revert() async {
        do {
            let _: Empty = try await APIClient.shared.delete("/api/ebooks/\(uuid)/overrides")
            await OfflineStore.shared.cacheDelete(CacheKey.book(uuid))
            Haptics.success()
            onSaved?()
            dismiss()
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}

/// A list-valued field: removable chips plus an add field. Authors and tags
/// share it — order is meaningful in both (the byline; the tag order the
/// server stores), so entries append and can be removed individually rather
/// than being re-typed as one delimited string.
private struct ChipListField: View {
    let label: String
    let placeholder: String
    @Binding var values: [String]
    @Binding var entry: String
    var isEdited = false
    /// Tags refuse an exact re-add (ignoring case) the way the web chip
    /// editor does; authors keep duplicates because two credited
    /// contributors can legitimately share a name.
    var deduplicates = false
    /// First row of its plate — no leading hairline.
    var isFirst = false
    /// Autocomplete pool for the entry field. Empty → free-text entry only,
    /// same as the web chip editor.
    var suggestions: [SuggestionItem] = []

    @Environment(\.palette) private var palette
    @FocusState private var entryFocused: Bool
    /// Whether the dropdown may show. Tracks focus, but stays closed after a
    /// commit until the next keystroke or refocus — mirroring the web
    /// editor's `suppress_open`, so the just-emptied entry doesn't instantly
    /// re-surface the pool.
    @State private var open = false

    var body: some View {
        VStack(spacing: 0) {
            if !isFirst {
                Rectangle()
                    .fill(palette.line2.color)
                    .frame(height: 0.5)
            }

            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 6) {
                    Text(label.uppercased())
                        .font(.monoUI(10, weight: .medium))
                        .tracking(0.7)
                        .foregroundStyle(isEdited ? palette.accentColor : palette.ink3Color)

                    if isEdited {
                        Circle()
                            .fill(palette.accentColor)
                            .frame(width: 4, height: 4)
                            .transition(.scale.combined(with: .opacity))
                    }

                    Spacer(minLength: 0)
                }

                if !values.isEmpty {
                    FlowLayout(spacing: 6, lineSpacing: 6) {
                        // Index-identified: authors can legitimately repeat a
                        // name, and the name is also what changes.
                        ForEach(Array(values.enumerated()), id: \.offset) { index, name in
                            chip(name, at: index)
                        }
                    }
                }

                HStack(spacing: 7) {
                    Image(systemName: "plus.circle")
                        .font(.system(size: 14))
                        .foregroundStyle(palette.ink3Color)

                    TextField(placeholder, text: $entry)
                        .font(.ui(15))
                        .foregroundStyle(palette.ink0Color)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                        .submitLabel(.done)
                        .tint(palette.accentColor)
                        .focused($entryFocused)
                        .onSubmit(commit)

                    if !entry.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Button("Add", action: commit)
                            .font(.ui(13, weight: .semibold))
                            .foregroundStyle(palette.accentColor)
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .animation(Motion.snap, value: isEdited)
            .animation(Motion.snap, value: values)

            if open {
                let rows = SuggestionPool.filtered(
                    pool: suggestions, current: values, query: entry
                )
                let trimmed = entry.trimmingCharacters(in: .whitespacesAndNewlines)
                let create = SuggestionPool.showsCreateRow(
                    pool: suggestions, current: values, typed: entry
                ) ? trimmed : nil
                if !rows.isEmpty || create != nil {
                    SuggestionList(items: rows, createText: create) { pick($0) }
                        .padding(.bottom, 6)
                }
            }
        }
        .onChange(of: entryFocused) { _, focused in
            open = focused
        }
        .onChange(of: entry) { _, newValue in
            // Only a keystroke reopens: the commit path clears the entry
            // programmatically, and that clear must not resurface the pool.
            if !newValue.isEmpty { open = true }
        }
    }

    private func chip(_ name: String, at index: Int) -> some View {
        HStack(spacing: 5) {
            Text(name)
                .font(.ui(13, weight: .medium))
                .foregroundStyle(palette.ink1Color)

            Button {
                Haptics.tap()
                values.remove(at: index)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(palette.ink3Color)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(name)")
        }
        .padding(.leading, 11)
        .padding(.trailing, 8)
        .padding(.vertical, 6)
        .background(Capsule().fill(palette.bg2Color))
        .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
    }

    private func commit() {
        pick(entry)
    }

    /// Commit `name` as a chip — from the entry field, a suggestion row, or
    /// the "+ Create" row. The entry always clears and the dropdown closes:
    /// a refused duplicate was still understood, and leaving the text (or
    /// the pool) sitting there reads as a failure.
    private func pick(_ name: String) {
        defer {
            entry = ""
            open = false
        }
        guard let chip = ChipEntry.committed(
            from: name, existing: values, deduplicating: deduplicates
        ) else { return }
        Haptics.select()
        values.append(chip)
    }
}
