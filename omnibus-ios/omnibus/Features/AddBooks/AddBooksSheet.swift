//  AddBooksSheet.swift
//  Entry point for adding books: upload a file from this device, or check in a
//  physical copy by scanning its barcode.

import SwiftUI
import UniformTypeIdentifiers

struct AddBooksSheet: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) private var app

    @State private var showFileImporter = false
    @State private var showCheckIn = false
    @State private var uploads: [UploadTask] = []

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: Spacing.md) {
                    if app.user?.canUpload == true {
                        actionCard(
                            icon: "arrow.up.doc",
                            title: "Upload a file",
                            subtitle: "Add an EPUB or audiobook from this device or iCloud Drive."
                        ) { showFileImporter = true }
                    }

                    actionCard(
                        icon: "barcode.viewfinder",
                        title: "Scan a barcode",
                        subtitle: "Check in a physical copy you own on paper."
                    ) { showCheckIn = true }

                    if !uploads.isEmpty {
                        VStack(alignment: .leading, spacing: Spacing.sm) {
                            Text("Uploads")
                                .font(.ui(13, weight: .semibold))
                                .foregroundStyle(palette.ink2Color)
                            ForEach(uploads) { task in
                                UploadRow(task: task)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, Spacing.md)
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
            }
            .background(ScreenBackground())
            .navigationTitle("Add books")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .sheet(isPresented: $showCheckIn) { CheckInView() }
        .fileImporter(
            isPresented: $showFileImporter,
            allowedContentTypes: Self.acceptedTypes,
            allowsMultipleSelection: true
        ) { result in
            guard case let .success(urls) = result else { return }
            for url in urls { upload(url) }
        }
    }

    private static var acceptedTypes: [UTType] {
        var types: [UTType] = [.epub, .audio, .mpeg4Audio, .mp3, .data]
        if let m4b = UTType(filenameExtension: "m4b") { types.append(m4b) }
        return types
    }

    private func actionCard(
        icon: String, title: String, subtitle: String, action: @escaping () -> Void
    ) -> some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            HStack(spacing: Spacing.md) {
                Image(systemName: icon)
                    .font(.system(size: 20, weight: .light))
                    .foregroundStyle(palette.accentColor)
                    .frame(width: 38)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.ui(15.5, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                    Text(subtitle)
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink3Color)
                        .multilineTextAlignment(.leading)
                }
                Spacer(minLength: 0)
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

    private func upload(_ url: URL) {
        let task = UploadTask(name: url.lastPathComponent)
        uploads.append(task)

        Task {
            // Security-scoped access is required for anything the picker
            // returns from outside the app container.
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }

            guard let data = try? Data(contentsOf: url) else {
                task.finish(error: "Couldn't read that file.")
                return
            }

            let isAudio = Book.audioFormats.contains(url.pathExtension.lowercased())
            let endpoint = isAudio ? "/api/uploads/audiobooks" : "/api/uploads/ebooks"
            do {
                let _: Empty = try await APIClient.shared.upload(
                    endpoint,
                    fileData: data,
                    fileName: url.lastPathComponent,
                    mimeType: isAudio ? "audio/mp4" : "application/epub+zip"
                )
                task.finish(error: nil)
                Haptics.success()
            } catch {
                task.finish(
                    error: (error as? APIError)?.errorDescription ?? error.localizedDescription
                )
            }
        }
    }
}

@Observable
@MainActor
final class UploadTask: Identifiable {
    let id = UUID()
    let name: String
    var isFinished = false
    var error: String?

    init(name: String) {
        self.name = name
    }

    func finish(error: String?) {
        self.error = error
        isFinished = true
    }
}

private struct UploadRow: View {
    let task: UploadTask
    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: Spacing.sm) {
            if !task.isFinished {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: task.error == nil ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                    .foregroundStyle(task.error == nil ? palette.okColor : palette.badColor)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(task.name)
                    .font(.ui(13))
                    .foregroundStyle(palette.ink1Color)
                    .lineLimit(1)
                if let error = task.error {
                    Text(error)
                        .font(.ui(11))
                        .foregroundStyle(palette.badColor)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6)
    }
}
