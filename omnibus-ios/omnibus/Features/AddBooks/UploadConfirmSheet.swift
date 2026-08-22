//  UploadConfirmSheet.swift
//  The confirm step of an upload: the metadata the server read out of the file,
//  editable, before it is filed.
//
//  Title and author are required — the commit handlers reject a body without
//  them, and the pair also names the folder the book is filed into — so Add
//  stays disabled until both are filled rather than letting the server answer.

import SwiftUI

struct UploadConfirmSheet: View {
    let draft: UploadDraft

    @Environment(\.palette) private var palette

    @State private var title = ""
    @State private var author = ""
    @State private var series = ""
    @State private var seriesIndex = ""
    /// Seeded once from the inspection — re-seeding on every render would
    /// stomp the reader's edits.
    @State private var seeded = false
    private var manager: UploadManager { UploadManager.shared }
    private var isOnline: Bool { Connectivity.shared.isOnline }

    /// The sheet stays on screen and hit-testable for the whole dismissal
    /// animation, so both buttons have to stand down once Add is tapped: a
    /// second Add commits the draft twice, and a Cancel deletes the staged
    /// bytes out from under the commit that is already reading them.
    private var isSubmitting: Bool { manager.isSubmitting }

    /// An upload is never queued (rule 08), so the write itself is gated, not
    /// just the flow's entry: the confirm step puts an unbounded human pause
    /// between picking a file and committing it, and the reader can lose
    /// connectivity inside that window.
    private var canCommit: Bool {
        UploadFlow.canCommit(
            kind: draft.batch.kind,
            title: title,
            author: author,
            series: series,
            seriesIndex: seriesIndex
        ) && isOnline && !isSubmitting
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.lg) {
                    header
                    field("Title", text: $title, identifier: "upload-title")
                    field("Author", text: $author, identifier: "upload-author")
                    if draft.batch.kind.acceptsSeries {
                        field("Series (optional)", text: $series, identifier: "upload-series")
                        field(
                            "Book # (optional)",
                            text: $seriesIndex,
                            identifier: "upload-series-index"
                        )
                    }
                }
                .screenPadding()
                .padding(.vertical, Spacing.lg)
            }
            .background(ScreenBackground())
            .navigationTitle("Confirm details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { manager.cancelActive() }
                        .disabled(isSubmitting)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") {
                        Haptics.tap()
                        manager.confirm(
                            UploadConfirmation(
                                title: title,
                                author: author,
                                series: series,
                                seriesIndex: seriesIndex
                            )
                        )
                    }
                    .disabled(!canCommit)
                }
            }
        }
        .presentationDetents([.medium, .large])
        // Swiping the sheet away would leave its row stuck on "waiting for
        // you" with a staged copy nobody deletes — Cancel is the way out.
        .interactiveDismissDisabled()
        .onAppear {
            guard !seeded else { return }
            seeded = true
            title = draft.confirmation.title
            author = draft.confirmation.author
            series = draft.confirmation.series
            seriesIndex = draft.confirmation.seriesIndex
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(draft.batch.displayName)
                .font(.display(20))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(2)
            Text(
                "We read these from the file. Correct anything that's off — the title and "
                    + "author decide where the book is filed in your library."
            )
            .font(.ui(13.5))
            .foregroundStyle(palette.ink2Color)
            if !isOnline {
                Text("You're offline — adding a book needs a connection.")
                    .font(.ui(12.5))
                    .foregroundStyle(palette.badColor)
            }
        }
    }

    private func field(
        _ label: String, text: Binding<String>, identifier: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.ui(12.5, weight: .semibold))
                .foregroundStyle(palette.ink2Color)
            TextField(label, text: text)
                .textFieldStyle(OmnibusFieldStyle())
                .autocorrectionDisabled()
                .accessibilityIdentifier(identifier)
        }
    }
}
