//  AddBooksSheet.swift
//  Entry point for adding books: upload a file from this device, or check in a
//  physical copy by scanning its barcode.
//
//  An upload is the server's two-step ingest — inspect the file for its
//  embedded metadata, let the reader correct it, then commit — because the
//  commit endpoints file the book into a folder named from the confirmed
//  title and author and reject a body that carries neither.
//
//  The queue, the staged copies and the transfers belong to `UploadManager`,
//  not to this sheet: they outlive a dismissal, and when the view owned them
//  every exit path had to remember to clean up.

import SwiftUI

struct AddBooksSheet: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) private var app

    @State private var showFileImporter = false
    @State private var showCheckIn = false

    private var manager: UploadManager { UploadManager.shared }
    private var isOnline: Bool { Connectivity.shared.isOnline }

    /// Presentation is driven by the manager; the setter only reports the
    /// dismissal back, so the next draft is never armed mid-animation.
    private var activeDraft: Binding<UploadDraft?> {
        Binding(
            get: { manager.activeDraft },
            set: { if $0 == nil { manager.sheetDidDismiss() } }
        )
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: Spacing.md) {
                    if app.user?.canUpload == true {
                        actionCard(
                            icon: "arrow.up.doc",
                            title: "Upload a file",
                            subtitle: isOnline
                                ? "Add an EPUB or audiobook from this device or iCloud Drive."
                                : "Unavailable offline — uploads go straight to your library."
                        ) { showFileImporter = true }
                        // An upload is library-wide state, not per-user, so
                        // it is never queued (rule 08, test 1) — the control
                        // stands down instead of failing after the fact.
                        .disabled(!isOnline)
                        .opacity(isOnline ? 1 : 0.4)
                    }

                    actionCard(
                        icon: "barcode.viewfinder",
                        title: "Scan a barcode",
                        subtitle: "Check in a physical copy you own on paper."
                    ) { showCheckIn = true }

                    if !manager.unsupported.isEmpty { unsupportedNotice }
                    if !manager.uploads.isEmpty { uploadList }
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
        // Staging from runs that are over — a crash, or a kill mid-confirm.
        // iOS does not purge tmp while the app runs, so nothing else reclaims
        // it; the manager excludes anything it still owns.
        .onAppear { manager.sweepAbandonedStaging() }
        .sheet(isPresented: $showCheckIn) { CheckInView() }
        .sheet(item: activeDraft) { draft in
            UploadConfirmSheet(draft: draft)
        }
        .fileImporter(
            isPresented: $showFileImporter,
            allowedContentTypes: UploadFlow.pickerTypes,
            allowsMultipleSelection: true
        ) { result in
            guard case let .success(urls) = result else { return }
            manager.enqueue(urls)
        }
    }

    private var uploadList: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text("Uploads")
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(palette.ink2Color)
            ForEach(manager.uploads) { task in
                UploadRow(task: task)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, Spacing.md)
    }

    private var unsupportedNotice: some View {
        Text(
            "Can't add \(manager.unsupported.joined(separator: ", ")) — books must be an "
                + "EPUB, and audiobooks an M4B, M4A, or MP3. An MP3 saved as .mpga needs "
                + "renaming first."
        )
        .font(.ui(12.5))
        .foregroundStyle(palette.ink3Color)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, Spacing.sm)
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
}

/// One inspected upload awaiting confirmation: the staged files, where they
/// live, and the metadata the server read out of them.
struct UploadDraft: Identifiable {
    let id = UUID()
    var batch: UploadBatch
    var directory: URL
    var confirmation: UploadConfirmation
    /// The progress row this draft reports into.
    var task: UploadTask
}

@Observable
@MainActor
final class UploadTask: Identifiable {
    enum State: Equatable {
        case inspecting
        case needsDetails
        case uploading
        case finished
        case failed(String)
    }

    let id = UUID()
    let name: String
    var state: State = .inspecting

    init(name: String) {
        self.name = name
    }
}

private struct UploadRow: View {
    let task: UploadTask
    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: Spacing.sm) {
            switch task.state {
            case .inspecting, .uploading:
                ProgressView().controlSize(.small)
            case .needsDetails:
                Image(systemName: "square.and.pencil")
                    .foregroundStyle(palette.ink2Color)
            case .finished:
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(palette.okColor)
            case .failed:
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(palette.badColor)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(task.name)
                    .font(.ui(13))
                    .foregroundStyle(palette.ink1Color)
                    .lineLimit(1)
                if let detail {
                    Text(detail)
                        .font(.ui(11))
                        .foregroundStyle(
                            isFailed ? palette.badColor : palette.ink3Color
                        )
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6)
    }

    private var isFailed: Bool {
        if case .failed = task.state { return true }
        return false
    }

    private var detail: String? {
        switch task.state {
        case .inspecting: "Reading details…"
        case .needsDetails: "Waiting for you to confirm the details"
        case .uploading: "Adding to your library…"
        case .finished: nil
        case let .failed(message): message
        }
    }
}
