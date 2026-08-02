//  BookContextMenu.swift
//  Long-press quick actions for a book cell: jump straight into the metadata
//  editor without the trip through the detail screen. One modifier shared by
//  every surface that renders a book cell, so a held-down book means the same
//  thing everywhere.

import SwiftUI

extension View {
    /// Attach the shared long-press menu to a book cell. `onEdited` runs after
    /// the editor saves or reverts (not on a cancel), so the surface can
    /// refresh the rows behind the sheet. `extras` appends surface-specific
    /// items — e.g. the shelf grid's "Remove from shelf".
    func bookContextMenu<Extras: View>(
        _ book: Book,
        onEdited: (() -> Void)? = nil,
        @ViewBuilder extras: @escaping () -> Extras = { EmptyView() }
    ) -> some View {
        modifier(BookContextMenuModifier(book: book, onEdited: onEdited, extras: extras))
    }
}

private struct BookContextMenuModifier<Extras: View>: ViewModifier {
    let book: Book
    var onEdited: (() -> Void)?
    @ViewBuilder var extras: () -> Extras

    @Environment(AppState.self) private var app
    @State private var showEditor = false

    func body(content: Content) -> some View {
        content
            .contextMenu {
                if app.user?.canEdit == true {
                    Button {
                        showEditor = true
                    } label: {
                        Label("Edit metadata", systemImage: "pencil")
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
