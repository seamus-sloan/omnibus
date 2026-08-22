//  AddBooksSheet.swift
//  Entry point for adding books: upload a file from this device, or check in a
//  physical copy by scanning its barcode.
//
//  An upload is the server's two-step ingest — inspect the file for its
//  embedded metadata, let the reader correct it, then commit — because the
//  commit endpoints file the book into a folder named from the confirmed
//  title and author and reject a body that carries neither.

import SwiftUI

struct AddBooksSheet: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) private var app

    @State private var showFileImporter = false
    @State private var showCheckIn = false
    @State private var uploads: [UploadTask] = []
    /// Inspected uploads waiting for their confirm sheet, shown one at a time
    /// so a multi-file pick doesn't stack sheets.
    @State private var pending: [UploadDraft] = []
    @State private var activeDraft: UploadDraft?
    /// Names the picker allowed through that neither endpoint accepts.
    @State private var unsupported: [String] = []
    /// The in-flight inspect pipeline, held so dismissal can cancel it rather
    /// than let it write into torn-down `@State`.
    @State private var pipeline: Task<Void, Never>?

    private var isOnline: Bool { Connectivity.shared.isOnline }

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
                        // An upload is a command against a shared library, so
                        // it is never queued (rule 08) — the control stands
                        // down instead of failing after the fact.
                        .disabled(!isOnline)
                        .opacity(isOnline ? 1 : 0.4)
                    }

                    actionCard(
                        icon: "barcode.viewfinder",
                        title: "Scan a barcode",
                        subtitle: "Check in a physical copy you own on paper."
                    ) { showCheckIn = true }

                    if !unsupported.isEmpty { unsupportedNotice }
                    if !uploads.isEmpty { uploadList }
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
        // Anything still staged belongs to a run that is over — a crash, or a
        // dismissal that outran its own cleanup. iOS does not purge tmp while
        // the app runs, so nothing else would ever reclaim it.
        .onAppear { UploadService.sweepStaleStaging() }
        .onDisappear(perform: tearDown)
        .sheet(isPresented: $showCheckIn) { CheckInView() }
        .sheet(item: $activeDraft, onDismiss: presentNextDraft) { draft in
            UploadConfirmSheet(
                draft: draft,
                onCommit: { commit(draft, as: $0) },
                onCancel: { cancel(draft) }
            )
        }
        .fileImporter(
            isPresented: $showFileImporter,
            allowedContentTypes: UploadFlow.pickerTypes,
            allowsMultipleSelection: true
        ) { result in
            guard case let .success(urls) = result else { return }
            stage(urls)
        }
    }

    private var uploadList: some View {
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

    private var unsupportedNotice: some View {
        Text(
            "Can't add \(unsupported.joined(separator: ", ")) — books must be an EPUB, "
                + "and audiobooks an M4B, M4A, or MP3."
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

    // MARK: - Upload pipeline

    /// Group the pick into one batch per book, then inspect them in turn.
    ///
    /// Sequentially, not concurrently: the confirm sheets are shown one at a
    /// time anyway, so a fan-out only front-loads a whole file's worth of
    /// memory per batch that the reader cannot act on yet.
    private func stage(_ urls: [URL]) {
        let selection = UploadFlow.selection(for: urls)
        unsupported = selection.unsupported
        guard !selection.batches.isEmpty else { return }

        let queued = selection.batches.map { batch in
            let task = UploadTask(name: batch.displayName)
            uploads.append(task)
            return (batch, task)
        }
        let previous = pipeline
        pipeline = Task {
            await previous?.value
            for (batch, task) in queued {
                if Task.isCancelled {
                    task.state = .failed("Cancelled.")
                    continue
                }
                await inspect(batch, into: task)
            }
        }
    }

    private func inspect(_ batch: UploadBatch, into task: UploadTask) async {
        var staging: URL?
        do {
            let (staged, directory) = try await UploadService.stage(batch)
            staging = directory
            let confirmation = try await UploadService.inspect(staged)
            task.state = .needsDetails
            pending.append(
                UploadDraft(
                    batch: staged, directory: directory, confirmation: confirmation, task: task
                )
            )
            presentNextDraft()
        } catch {
            task.state = .failed(Self.message(for: error))
            // The copy is already on disk by the time inspect can fail, and
            // only commit/cancel discard — without this it is unreachable.
            if let staging { UploadService.discard(staging) }
        }
    }

    /// Cancel in-flight work and release every staged copy the sheet still
    /// owns. Tapping Done drops `pending` and `activeDraft` on the floor, so
    /// the bytes they point at have to go with them.
    private func tearDown() {
        pipeline?.cancel()
        pipeline = nil
        for draft in pending { UploadService.discard(draft.directory) }
        pending.removeAll()
        if let activeDraft {
            UploadService.discard(activeDraft.directory)
            self.activeDraft = nil
        }
    }

    /// Show the next inspected upload's confirm sheet, if one isn't already up.
    private func presentNextDraft() {
        guard activeDraft == nil, !pending.isEmpty else { return }
        activeDraft = pending.removeFirst()
    }

    private func commit(_ draft: UploadDraft, as confirmation: UploadConfirmation) {
        draft.task.state = .uploading
        activeDraft = nil
        Task {
            do {
                _ = try await UploadService.commit(draft.batch, as: confirmation)
                draft.task.state = .finished
                Haptics.success()
            } catch {
                draft.task.state = .failed(Self.message(for: error))
            }
            UploadService.discard(draft.directory)
            // No presentNextDraft here: `onDismiss` already drains the queue on
            // both exit paths, and re-arming `activeDraft` mid-dismissal is
            // silently dropped by `sheet(item:)` — which then wedges the queue
            // forever behind the `activeDraft == nil` guard.
        }
    }

    /// Drop an upload the reader backed out of: no row is left behind, and the
    /// staged copy goes with it.
    private func cancel(_ draft: UploadDraft) {
        uploads.removeAll { $0.id == draft.task.id }
        UploadService.discard(draft.directory)
        activeDraft = nil
    }

    private static func message(for error: Error) -> String {
        (error as? APIError)?.errorDescription ?? error.localizedDescription
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
