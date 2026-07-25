//  SettingsView.swift
//  Admin-only server settings: library paths and index maintenance.

import SwiftUI

struct SettingsView: View {
    @Environment(\.palette) private var palette

    @State private var ebookPath = ""
    @State private var audiobookPath = ""
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var status: String?
    @State private var isError = false

    var body: some View {
        Group {
            if isLoading {
                LoadingView()
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 28) {
                        paths
                        maintenance
                        if let status { statusLine(status) }
                    }
                    .screenPadding()
                    .padding(.top, Spacing.md)
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
                .scrollDismissesKeyboard(.interactively)
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Server settings")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
    }

    /// A filesystem path is long, monospaced, and edited whole — a trailing
    /// `LabeledContent` field gave it a third of the row and truncated the part
    /// that identifies it.
    private var paths: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Library paths")

            Plate {
                PlateField(
                    label: "Ebooks", text: $ebookPath, isFirst: true,
                    hint: "/path/to/ebooks"
                )
                PlateField(
                    label: "Audiobooks", text: $audiobookPath,
                    hint: "/path/to/audiobooks"
                )
            }

            Button {
                Task { await save() }
            } label: {
                if isSaving {
                    ProgressView().tint(palette.accentInk.color)
                } else {
                    Text("Save")
                }
            }
            .buttonStyle(FilledButtonStyle())
            .disabled(isSaving)

            Text("Saving repoints the library and reindexes in the background.")
                .font(.ui(12))
                .foregroundStyle(palette.ink3Color)
        }
    }

    private var maintenance: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            SectionLabel("Maintenance")

            VStack(spacing: 0) {
                taskRow(
                    "Reindex library", detail: "Re-read every file's metadata.",
                    icon: "arrow.triangle.2.circlepath", isFirst: true
                ) {
                    await post("/api/reindex", "Reindex started.")
                }
                taskRow(
                    "Scan library", detail: "Look for files added or removed on disk.",
                    icon: "magnifyingglass"
                ) {
                    await post("/api/scan-library", "Scan started.")
                }
                taskRow(
                    "Rebuild search index", detail: "Recreate the full-text index.",
                    icon: "text.magnifyingglass"
                ) {
                    await post("/api/fts/rebuild", "Search index rebuilt.")
                }
                RecordRule()
            }
        }
    }

    /// Each job says what it will do — the names alone don't distinguish a
    /// reindex from a scan, and both are slow enough to be worth choosing well.
    private func taskRow(
        _ title: String, detail: String, icon: String,
        isFirst: Bool = false, action: @escaping () async -> Void
    ) -> some View {
        Button {
            Haptics.tap()
            Task { await action() }
        } label: {
            VStack(spacing: 0) {
                if !isFirst { Hairline() }

                HStack(spacing: Spacing.md) {
                    Image(systemName: icon)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(palette.accentColor)
                        .frame(width: 24)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(title)
                            .font(.ui(15))
                            .foregroundStyle(palette.ink0Color)
                        Text(detail)
                            .font(.ui(11.5))
                            .foregroundStyle(palette.ink3Color)
                            .multilineTextAlignment(.leading)
                    }

                    Spacer(minLength: 0)
                }
                .padding(.vertical, 12)
                .contentShape(Rectangle())
            }
        }
        .buttonStyle(PressableStyle())
    }

    private func statusLine(_ status: String) -> some View {
        Label(status, systemImage: isError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
            .font(.ui(13))
            .foregroundStyle(isError ? palette.badColor : palette.okColor)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(Spacing.md)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill((isError ? palette.badColor : palette.okColor).opacity(0.12))
            )
            .transition(.opacity.combined(with: .move(edge: .top)))
    }

    private func load() async {
        if let settings: LibrarySettings = try? await APIClient.shared.get("/api/settings") {
            ebookPath = settings.ebookLibraryPath ?? ""
            audiobookPath = settings.audiobookLibraryPath ?? ""
        }
        isLoading = false
    }

    private func save() async {
        isSaving = true
        defer { isSaving = false }
        let body = LibrarySettings(
            ebookLibraryPath: ebookPath.nilIfBlank,
            audiobookLibraryPath: audiobookPath.nilIfBlank
        )
        do {
            let _: Empty = try await APIClient.shared.post("/api/settings", body: body)
            // The reindex runs in a background task server-side, so the save
            // returning is not the same as the library being current.
            show("Saved. Reindexing in the background.", error: false)
        } catch {
            show((error as? APIError)?.errorDescription ?? error.localizedDescription, error: true)
        }
    }

    private func post(_ path: String, _ success: String) async {
        do {
            let _: Empty = try await APIClient.shared.post(path, body: Empty())
            show(success, error: false)
        } catch {
            show((error as? APIError)?.errorDescription ?? error.localizedDescription, error: true)
        }
    }

    private func show(_ message: String, error: Bool) {
        withAnimation {
            status = message
            isError = error
        }
        error ? Haptics.warning() : Haptics.success()
    }
}
