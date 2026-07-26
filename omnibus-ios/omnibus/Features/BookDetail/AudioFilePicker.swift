//  AudioFilePicker.swift
//  Choosing which audiobook file of a book to listen to — a book can carry
//  more than one (two narrations, an abridged edition), and they don't share
//  a timeline.

import SwiftUI

/// Lists a book's audiobook files and reports the one tapped. The caller
/// presents the player from its own `onDismiss` — presenting a full-screen
/// cover while this sheet is still up would be refused.
struct AudioFilePickerSheet: View {
    let book: Book
    let onPick: (BookFileInfo) -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    /// The file the saved listening position was taken in, so the row can
    /// say which choice continues and which starts over. Read from the
    /// replica only — this must answer instantly, offline included.
    @State private var inProgressFileID: Int64?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: Spacing.sm) {
                    ForEach(book.audioFiles) { file in
                        fileRow(file)
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
            }
            .background(ScreenBackground())
            .navigationTitle("Choose a file")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task {
            let saved = await UserDataService.localProgress(uuid: book.uuid, format: .audio)
            inProgressFileID = saved?.bookFileID
        }
    }

    private func fileRow(_ file: BookFileInfo) -> some View {
        Button {
            Haptics.tap()
            onPick(file)
            dismiss()
        } label: {
            HStack(spacing: Spacing.md) {
                Image(systemName: "headphones")
                    .font(.system(size: 20, weight: .light))
                    .foregroundStyle(palette.accentColor)
                    .frame(width: 38)

                VStack(alignment: .leading, spacing: 3) {
                    Text(file.label?.nilIfBlank ?? file.filename)
                        .font(.ui(15.5, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                    Text(detail(for: file))
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink3Color)
                }

                Spacer(minLength: 0)

                if file.id == inProgressFileID {
                    Text("In progress")
                        .font(.ui(11, weight: .semibold))
                        .foregroundStyle(palette.accentColor)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Capsule().fill(palette.accentColor.opacity(0.14)))
                }

                Image(systemName: "chevron.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink3Color)
            }
            .padding(Spacing.md)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(palette.bg1Color)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 0.5)
            )
        }
        .buttonStyle(PressableStyle())
    }

    private func detail(for file: BookFileInfo) -> String {
        var parts = [file.format.uppercased()]
        if file.sizeBytes > 0 { parts.append(Format.bytes(file.sizeBytes)) }
        return parts.joined(separator: " · ")
    }
}
