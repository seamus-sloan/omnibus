//  MetadataEditView.swift
//  Metadata override editor.
//
//  Built as an editable colophon rather than a `Form`: the detail screen states
//  a book's printing details as mono small-caps keys over the reading face, and
//  this is the same plate with the values made editable. A stock `Form` also
//  used each field's placeholder as its only label, so a filled row lost its
//  name — "en" with nothing to say it meant Language.

import SwiftUI

/// Every editable value on the screen, as one comparable value.
///
/// Kept as a single struct rather than nine `@State` strings so "has anything
/// changed" is a plain `!=` against the loaded snapshot — which is what lets
/// Save disable itself until there's something to save, and what drives the
/// per-field edited markers.
private struct MetadataDraft: Equatable {
    var title = ""
    /// A real list, not a comma-joined string: authors are structured on the
    /// wire, and asking someone to maintain delimiters by hand made the one
    /// genuinely multi-value field the most error-prone on the screen.
    var authors: [String] = []
    var series = ""
    var seriesIndex = ""
    var publisher = ""
    var published = ""
    var language = ""
    var isbn13 = ""
    var description = ""
}

struct MetadataEditView: View {
    let uuid: String

    init(uuid: String) {
        self.uuid = uuid
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
    /// Held here rather than inside `AuthorsField` so saving can flush it.
    @State private var pendingAuthor = ""

    private var pendingAuthorName: String {
        pendingAuthor.trimmingCharacters(in: .whitespaces)
    }

    private var isDirty: Bool { draft != loaded || !pendingAuthorName.isEmpty }

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
    }

    private var content: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                if let book { header(book) }

                group("Title & authors") {
                    Plate {
                        field("Title", \.title, isFirst: true)
                        AuthorsField(
                            authors: $draft.authors,
                            entry: $pendingAuthor,
                            isEdited: draft.authors != loaded.authors
                        )
                    }
                }

                group("Series") {
                    Plate {
                        field("Series", \.series, isFirst: true)
                        field("Index", \.seriesIndex, keyboard: .decimalPad)
                    }
                }

                group("Publication") {
                    Plate {
                        field("Publisher", \.publisher, isFirst: true)
                        field("Published", \.published, hint: "YYYY-MM-DD")
                        field("Language", \.language, hint: "e.g. en")
                        field("ISBN-13", \.isbn13, keyboard: .numberPad)
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
        multiline: Bool = false
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
            multiline: multiline
        )
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
        book = try? await LibraryService.book(uuid: uuid)
        guard let book else {
            isLoading = false
            return
        }
        loaded = MetadataDraft(
            title: book.title ?? "",
            authors: book.creators.map(\.name),
            series: book.series ?? "",
            seriesIndex: book.seriesIndex ?? "",
            publisher: book.publisher ?? "",
            published: book.published ?? "",
            language: book.language ?? "",
            isbn13: book.isbn13 ?? "",
            description: book.description ?? ""
        )
        draft = loaded
        isLoading = false
    }

    /// A field's value, but only when the user actually changed it.
    ///
    /// Sending every field on every save wrote overrides for fields nobody
    /// touched, pinning scanned values so a later rescan could no longer update
    /// them. The endpoint merges, so omitting a field leaves it as it was, and
    /// an empty string clears an existing override.
    private func changed(_ key: KeyPath<MetadataDraft, String>) -> String? {
        let value = draft[keyPath: key]
        return value == loaded[keyPath: key] ? nil : value
    }

    private func save() async {
        isSaving = true
        error = nil
        defer { isSaving = false }

        // A name typed into the add field but never committed to a chip is
        // still what the user meant to save; dropping it silently is worse
        // than accepting it.
        if !pendingAuthorName.isEmpty {
            draft.authors.append(pendingAuthorName)
            pendingAuthor = ""
        }

        // `creators` is a list of Contributor *objects* on the wire, not bare
        // strings: sending `["E. M. Forster"]` fails the server's JSON
        // extraction outright, so every save with a non-empty Authors field
        // was rejected before it reached validation.
        struct Creator: Encodable {
            var name: String
        }

        struct Overrides: Encodable {
            var title: String?
            var creators: [Creator]?
            var series: String?
            var series_index: String?
            var publisher: String?
            var published: String?
            var language: String?
            var isbn13: String?
            var description: String?
        }

        let payload = Overrides(
            title: changed(\.title),
            creators: draft.authors == loaded.authors ? nil : draft.authors.map(Creator.init),
            series: changed(\.series),
            series_index: changed(\.seriesIndex),
            publisher: changed(\.publisher),
            published: changed(\.published),
            language: changed(\.language),
            isbn13: changed(\.isbn13),
            description: changed(\.description)
        )

        do {
            let _: Empty = try await APIClient.shared.post("/api/ebooks/\(uuid)/overrides", body: payload)
            await OfflineStore.shared.cacheDelete(CacheKey.book(uuid))
            Haptics.success()
            dismiss()
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func revert() async {
        do {
            let _: Empty = try await APIClient.shared.delete("/api/ebooks/\(uuid)/overrides")
            await OfflineStore.shared.cacheDelete(CacheKey.book(uuid))
            Haptics.success()
            dismiss()
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}

/// Authors as removable chips plus an add field.
///
/// Order is meaningful (it's the byline), so entries append and can be removed
/// individually rather than being re-typed as one delimited string.
private struct AuthorsField: View {
    @Binding var authors: [String]
    @Binding var entry: String
    var isEdited = false

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(palette.line2.color)
                .frame(height: 0.5)

            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 6) {
                    Text("AUTHORS")
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

                if !authors.isEmpty {
                    FlowLayout(spacing: 6, lineSpacing: 6) {
                        // Index-identified: two contributors can legitimately
                        // share a name, and the name is also what changes.
                        ForEach(Array(authors.enumerated()), id: \.offset) { index, name in
                            chip(name, at: index)
                        }
                    }
                }

                HStack(spacing: 7) {
                    Image(systemName: "plus.circle")
                        .font(.system(size: 14))
                        .foregroundStyle(palette.ink3Color)

                    TextField("Add an author", text: $entry)
                        .font(.ui(15))
                        .foregroundStyle(palette.ink0Color)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                        .submitLabel(.done)
                        .tint(palette.accentColor)
                        .onSubmit(commit)

                    if !entry.trimmingCharacters(in: .whitespaces).isEmpty {
                        Button("Add", action: commit)
                            .font(.ui(13, weight: .semibold))
                            .foregroundStyle(palette.accentColor)
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .animation(Motion.snap, value: isEdited)
            .animation(Motion.snap, value: authors)
        }
    }

    private func chip(_ name: String, at index: Int) -> some View {
        HStack(spacing: 5) {
            Text(name)
                .font(.ui(13, weight: .medium))
                .foregroundStyle(palette.ink1Color)

            Button {
                Haptics.tap()
                authors.remove(at: index)
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
        let name = entry.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        Haptics.select()
        authors.append(name)
        entry = ""
    }
}
