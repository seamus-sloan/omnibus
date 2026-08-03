//  BookContextMenu.swift
//  Long-press quick actions for a book cell: jump straight into the metadata
//  editor without the trip through the detail screen. One modifier shared by
//  every surface that renders a book cell, so a held-down book means the same
//  thing everywhere.

import SwiftUI

/// The glyphs the shared menu draws, named so a test can prove they resolve:
/// `Image(systemName:)` neither warns nor fails on a name that doesn't exist,
/// so a bad one ships as a menu row with a hole where its icon should be.
enum BookMenuGlyph {
    static let detail = "info.circle"
    static let edit = "pencil"
}

extension View {
    /// Attach the shared long-press menu to a book cell. `onEdited` runs after
    /// the editor saves or reverts (not on a cancel), so the surface can
    /// refresh the rows behind the sheet. `onOpenDetail` adds a "View book
    /// details" item, for cells whose *tap* goes somewhere else — on a cell
    /// that already pushes the detail screen it would only restate the tap.
    /// `extras` appends surface-specific items — e.g. the shelf grid's
    /// "Remove from shelf".
    func bookContextMenu<Extras: View>(
        _ book: Book,
        onEdited: (() -> Void)? = nil,
        onOpenDetail: (() -> Void)? = nil,
        @ViewBuilder extras: @escaping () -> Extras = { EmptyView() }
    ) -> some View {
        modifier(
            BookContextMenuModifier(
                book: book, onEdited: onEdited, onOpenDetail: onOpenDetail, extras: extras
            )
        )
    }
}

private struct BookContextMenuModifier<Extras: View>: ViewModifier {
    let book: Book
    var onEdited: (() -> Void)?
    var onOpenDetail: (() -> Void)?
    @ViewBuilder var extras: () -> Extras

    @Environment(AppState.self) private var app
    @State private var showEditor = false

    func body(content: Content) -> some View {
        content
            .contextMenu {
                // First where it appears at all: on a card whose tap opens the
                // reader, reaching the detail screen is the whole reason the
                // menu is worth holding the card down for.
                if let onOpenDetail {
                    Button {
                        onOpenDetail()
                    } label: {
                        Label("View book details", systemImage: BookMenuGlyph.detail)
                    }
                }
                if app.user?.canEdit == true {
                    Button {
                        showEditor = true
                    } label: {
                        Label("Edit metadata", systemImage: BookMenuGlyph.edit)
                    }
                }
                extras()
            }
            // A sheet rather than a push: the surfaces this attaches to sit in
            // four different NavigationStacks, and a sheet needs none of their
            // paths — edit, save, and you're still where you were.
            .sheet(isPresented: $showEditor) {
                NavigationStack {
                    MetadataEditView(uuid: book.uuid, onSaved: onEdited)
                        .toolbar {
                            ToolbarItem(placement: .cancellationAction) {
                                Button("Cancel") { showEditor = false }
                            }
                        }
                }
            }
    }
}
