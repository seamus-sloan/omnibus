//  ReaderContentsSheet.swift
//  Contents, bookmarks, and notes behind one segmented sheet.
//
//  Apple Books puts all three on the same sheet because they answer the same
//  question — "take me somewhere in this book" — and splitting them across
//  separate buttons made you remember which chrome control held which list.
//  Every row here is a jump; the segments only change what you're jumping from.

import SwiftUI

struct ReaderContentsSheet: View {
    let book: Book
    let controller: ReaderController
    /// The reader's live list, so a highlight made a moment ago is here
    /// without waiting on a refetch.
    let highlights: [Highlight]
    /// Which segment the menu row that opened this sheet was asking for.
    var initialTab: Tab = .contents
    var onRemoveHighlight: (Highlight) -> Void
    /// Keeps the caller's badge honest after a delete in here.
    var onBookmarkCountChanged: (Int) -> Void = { _ in }

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var tab: Tab?
    @State private var bookmarks: [Bookmark] = []
    @State private var isLoadingBookmarks = true

    enum Tab: String, CaseIterable {
        case contents, bookmarks, notes

        var label: String {
            switch self {
            case .contents: "Contents"
            case .bookmarks: "Bookmarks"
            case .notes: "Notes"
            }
        }
    }

    /// `tab` stays nil until first render so `initialTab` can seed it without
    /// a second pass that visibly switches segments after the sheet is up.
    private var selectedTab: Binding<Tab> {
        Binding(get: { tab ?? initialTab }, set: { tab = $0 })
    }

    var body: some View {
        VStack(spacing: 0) {
            header

            Picker("View", selection: selectedTab) {
                ForEach(Tab.allCases, id: \.self) { tab in
                    Text(tab.label).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .screenPadding()
            .padding(.bottom, Spacing.md)

            Group {
                switch selectedTab.wrappedValue {
                case .contents: contentsList
                case .bookmarks: bookmarksList
                case .notes: notesList
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(ScreenBackground())
        // Nearly full height at rest: a contents list is something you scan,
        // and a half sheet spends most of its space on the page behind it.
        .presentationDetents([.large])
        .task { await loadBookmarks() }
    }

    /// Cover, title, and where you are — the same three things the book's own
    /// detail page leads with, so the sheet reads as part of this book rather
    /// than a generic list.
    private var header: some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            BookCover(identity: CoverIdentity(book), size: .sm, cornerRadius: 4)
                .frame(width: 46)

            VStack(alignment: .leading, spacing: 2) {
                Text(book.displayTitle)
                    .font(.display(20))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(2)
                if let position = positionLabel {
                    Text(position)
                        .font(.ui(13))
                        .foregroundStyle(palette.ink2Color)
                }
            }

            Spacer(minLength: Spacing.sm)

            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(palette.ink1Color)
                    .frame(width: 32, height: 32)
                    .background(Circle().fill(palette.bg2Color))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Done")
        }
        .screenPadding()
        .padding(.top, Spacing.lg)
        .padding(.bottom, Spacing.lg)
    }

    private var positionLabel: String? {
        guard let location = controller.location else { return nil }
        return location.hasPageNumbers
            ? "Page \(location.page) of \(location.totalPages)"
            : "\(location.pct)% through"
    }

    /// Jumping is the whole point of this sheet, so every row ends the same way.
    private func jump(to target: String) {
        controller.display(target)
        dismiss()
    }

    // MARK: - Contents

    private var currentLabel: String? { controller.location?.chapterName }

    @ViewBuilder
    private var contentsList: some View {
        if controller.toc.isEmpty {
            EmptyStateView(icon: "list.bullet", title: "No table of contents")
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(controller.toc.enumerated()), id: \.element.id) { index, item in
                            tocRow(item, isFirst: index == 0)
                                .id(item.id)
                        }
                        RecordRule()
                    }
                    .screenPadding()
                    .padding(.bottom, 32)
                }
                .scrollIndicators(.hidden)
                .onAppear {
                    guard let current = controller.toc.first(where: { $0.label == currentLabel })
                    else { return }
                    proxy.scrollTo(current.id, anchor: .center)
                }
            }
        }
    }

    /// Marks where you are and opens there. A contents list that doesn't say
    /// which chapter you're in makes you count — and in a book with fifty
    /// near-identical chapter titles counting is the only way to tell.
    ///
    /// Top-level entries carry the weight, since in most books those are the
    /// parts and the nested ones the chapters inside them.
    private func tocRow(_ item: TocItem, isFirst: Bool) -> some View {
        let isCurrent = item.label == currentLabel
        let isPart = item.level == 0

        return Button {
            jump(to: item.href)
        } label: {
            VStack(spacing: 0) {
                if !isFirst { Hairline() }

                HStack(alignment: .firstTextBaseline, spacing: Spacing.md) {
                    // The marker keeps its column on every row, so a current
                    // chapter doesn't shunt the text sideways.
                    Capsule()
                        .fill(isCurrent ? palette.accentColor : .clear)
                        .frame(width: 2.5, height: 15)

                    Text(item.label)
                        .font(.ui(15.5, weight: rowWeight(isCurrent: isCurrent, isPart: isPart)))
                        .foregroundStyle(isCurrent ? palette.accentColor : palette.ink1Color)
                        .multilineTextAlignment(.leading)
                        .padding(.leading, CGFloat(item.level) * 16)

                    Spacer(minLength: Spacing.sm)

                    if let page = item.page {
                        Text("\(page)")
                            .font(.ui(14))
                            .foregroundStyle(isCurrent ? palette.accentColor : palette.ink3Color)
                            .monospacedDigit()
                    }
                }
                .padding(.vertical, 13)
                .contentShape(Rectangle())
            }
        }
        .buttonStyle(PressableStyle())
    }

    private func rowWeight(isCurrent: Bool, isPart: Bool) -> Font.Weight {
        if isCurrent { return .semibold }
        return isPart ? .medium : .regular
    }

    // MARK: - Bookmarks

    @ViewBuilder
    private var bookmarksList: some View {
        if isLoadingBookmarks {
            LoadingView()
        } else if bookmarks.isEmpty {
            EmptyStateView(
                icon: "bookmark",
                title: "No bookmarks",
                message: "Tap the ribbon while reading to save your place."
            )
        } else {
            List {
                ForEach(bookmarks) { bookmark in
                    Button {
                        jump(to: bookmark.position)
                    } label: {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(bookmark.title?.nilIfBlank ?? "Saved position")
                                .font(.ui(15, weight: .medium))
                                .foregroundStyle(palette.ink0Color)
                            Text(Format.relative(unix: bookmark.createdAt))
                                .font(.ui(11))
                                .foregroundStyle(palette.ink3Color)
                        }
                    }
                    .listRowBackground(palette.bg1Color)
                }
                .onDelete { offsets in
                    Task { await deleteBookmarks(at: offsets) }
                }
            }
            .scrollContentBackground(.hidden)
        }
    }

    private func loadBookmarks() async {
        for await items in UserDataService.bookmarks(uuid: book.uuid).values() {
            bookmarks = items
            isLoadingBookmarks = false
            onBookmarkCountChanged(items.count)
        }
        isLoadingBookmarks = false
    }

    private func deleteBookmarks(at offsets: IndexSet) async {
        let doomed = offsets.map { bookmarks[$0] }
        bookmarks.remove(atOffsets: offsets)
        onBookmarkCountChanged(bookmarks.count)
        for bookmark in doomed {
            await UserDataService.deleteBookmark(bookmark)
        }
    }

    // MARK: - Notes

    @ViewBuilder
    private var notesList: some View {
        if highlights.isEmpty {
            EmptyStateView(
                icon: "highlighter",
                title: "No highlights",
                message: "Select any passage to highlight it or attach a note."
            )
        } else {
            List {
                ForEach(sortedHighlights) { highlight in
                    Button {
                        // Anchorless (Kobo-origin) rows have nowhere to jump.
                        if let cfiRange = highlight.epubCFIRange {
                            jump(to: cfiRange)
                        }
                    } label: {
                        highlightRow(highlight)
                    }
                    .listRowBackground(palette.bg1Color)
                }
                .onDelete { offsets in
                    for index in offsets {
                        onRemoveHighlight(sortedHighlights[index])
                    }
                }
            }
            .scrollContentBackground(.hidden)
        }
    }

    /// Newest first — the passage you just marked is the one you're most
    /// likely to be coming back for.
    private var sortedHighlights: [Highlight] {
        highlights.sorted { $0.createdAt > $1.createdAt }
    }

    private func highlightRow(_ highlight: Highlight) -> some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            Capsule()
                .fill(highlight.color.tint)
                .frame(width: 3)

            VStack(alignment: .leading, spacing: 5) {
                if let text = highlight.text?.nilIfBlank {
                    Text(text)
                        .font(.display(15))
                        .foregroundStyle(palette.ink1Color)
                        .lineLimit(3)
                        .multilineTextAlignment(.leading)
                }
                if let note = highlight.note?.nilIfBlank {
                    Text(note)
                        .font(.ui(12))
                        .foregroundStyle(palette.ink2Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
                Text(Format.relative(unix: highlight.createdAt))
                    .font(.ui(11))
                    .foregroundStyle(palette.ink3Color)
            }
        }
        .padding(.vertical, 2)
        .fixedSize(horizontal: false, vertical: true)
    }
}
