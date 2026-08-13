//  AlignmentSheet.swift
//  The cross-format alignment sheet: both lanes with chapter ticks and
//  position markers, the sequence-vs-narrations declaration for multi-file
//  audio, the anchor-match readout, and confirm/unlink. Sync stays off
//  until the user confirms here. Configuration-shaped writes (rule 08):
//  online-only, never queued — the confirm is disabled offline.

import SwiftUI

struct AlignmentSheet: View {
    let book: Book
    var onChanged: () -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.palette) private var palette
    private var connectivity = Connectivity.shared

    @State private var view: AlignmentView?
    @State private var error: String?
    @State private var busy = false
    @State private var mode: CrossFormatLinkMode = .sequence
    @State private var primary: Int64?

    init(book: Book, onChanged: @escaping () -> Void) {
        self.book = book
        self.onChanged = onChanged
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.lg) {
                    Text("Check that the mapped position lands about where it should, then confirm. Sync stays off until you do, and you can unlink any time.")
                        .font(.ui(13))
                        .foregroundStyle(palette.ink2Color)

                    if let view {
                        lanes(view)
                        legend(view)
                        if view.audioFiles.count > 1 { choice(view) }
                        matchNote(view)
                        actions(view)
                    } else if error == nil {
                        ProgressView().frame(maxWidth: .infinity)
                    }
                    if let error {
                        Text(error)
                            .font(.ui(13))
                            .foregroundStyle(palette.warnColor)
                    }
                }
                .screenPadding()
                .padding(.vertical, Spacing.lg)
            }
            .background(ScreenBackground())
            .navigationTitle("Position sync")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }.disabled(busy)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .task { await load() }
    }

    @MainActor
    private func load() async {
        do {
            let v = try await UserDataService.alignment(uuid: book.uuid)
            mode = v.link?.mode ?? .sequence
            primary = v.link?.primaryBookFileID ?? v.audioFiles.first?.bookFileID
            view = v
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    // MARK: Lanes

    private func lanes(_ view: AlignmentView) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            laneLabel(ebookLabel(view))
            lane(
                ticks: (view.ebook?.chapters ?? []).map { $0.percent / 100 },
                marker: view.reading?.percent.map { Double($0) / 100 },
                mapped: nil
            )
            laneLabel(audioLabel(view))
            lane(
                ticks: audioTicks(view),
                marker: listeningFraction(view),
                mapped: mappedFraction(view)
            )
        }
    }

    /// Reading fraction mapped through the served anchor pairs — the same
    /// interpolation the jump runs, so the preview cannot disagree.
    private func mappedFraction(_ view: AlignmentView) -> Double? {
        guard let pct = view.reading?.percent else { return nil }
        var frac = Double(pct) / 100
        var prev = (0.0, 0.0)
        for pair in view.anchorPairs + [[1.0, 1.0]] where pair.count == 2 {
            let (t, a) = (pair[0], pair[1])
            if frac <= t {
                if t - prev.0 < .ulpOfOne { return prev.1 }
                let k = (frac - prev.0) / (t - prev.0)
                frac = min(max(prev.1 + k * (a - prev.1), 0), 1)
                return frac
            }
            prev = (t, a)
        }
        return frac
    }

    private func laneLabel(_ text: String) -> some View {
        Text(text)
            .font(.ui(11))
            .foregroundStyle(palette.ink2Color)
            .textCase(.uppercase)
    }

    /// One horizontal lane: a track with chapter ticks, a solid position
    /// marker, and the dashed mapped-preview marker.
    private func lane(ticks: [Double], marker: Double?, mapped: Double?) -> some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 8)
                    .fill(palette.ink2Color.opacity(0.12))
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(palette.ink2Color.opacity(0.25), lineWidth: 1)
                ForEach(Array(ticks.enumerated()), id: \.offset) { _, tick in
                    Rectangle()
                        .fill(palette.ink2Color.opacity(0.4))
                        .frame(width: 1, height: 16)
                        .offset(x: geo.size.width * min(max(tick, 0), 1))
                }
                if let marker {
                    Rectangle()
                        .fill(palette.accentColor)
                        .frame(width: 2, height: 34)
                        .offset(x: geo.size.width * min(max(marker, 0), 1) - 1)
                }
                if let mapped {
                    Rectangle()
                        .fill(palette.accentColor.opacity(0.7))
                        .frame(width: 2, height: 34)
                        .offset(x: geo.size.width * min(max(mapped, 0), 1) - 1)
                        .overlay(alignment: .top) {
                            Text("\u{2248}")
                                .font(.ui(10))
                                .foregroundStyle(palette.accentColor)
                                .offset(
                                    x: geo.size.width * min(max(mapped, 0), 1) - 1,
                                    y: -14
                                )
                        }
                }
            }
        }
        .frame(height: 30)
    }

    private func ebookLabel(_ view: AlignmentView) -> String {
        guard let ebook = view.ebook else { return "Ebook · percent of text" }
        return "Ebook · \(ebook.chapters.count) chapters"
    }

    private func audioLabel(_ view: AlignmentView) -> String {
        let total = view.audioFiles.reduce(0.0) { $0 + $1.durationSeconds }
        let files = view.audioFiles.count
        let suffix = Format.duration(total)
        return files > 1 ? "Audiobook · \(files) files · \(suffix)" : "Audiobook · \(suffix)"
    }

    /// Chapter starts across the (mode-filtered) timeline as fractions.
    private func timelineFiles(_ view: AlignmentView) -> [AlignmentAudioFile] {
        switch mode {
        case .sequence: view.audioFiles
        case .narrations: view.audioFiles.filter { $0.bookFileID == primary }
        }
    }

    private func audioTicks(_ view: AlignmentView) -> [Double] {
        let files = timelineFiles(view)
        let total = files.reduce(0.0) { $0 + $1.durationSeconds }
        guard total > 0 else { return [] }
        var ticks: [Double] = []
        var start = 0.0
        for file in files {
            ticks.append(contentsOf: file.chapterStarts.map { (start + $0) / total })
            start += file.durationSeconds
        }
        return ticks
    }

    private func listeningFraction(_ view: AlignmentView) -> Double? {
        guard let pos = view.listening else { return nil }
        let files = timelineFiles(view)
        let total = files.reduce(0.0) { $0 + $1.durationSeconds }
        guard total > 0 else { return nil }
        let fileID = pos.bookFileID ?? (files.count == 1 ? files.first?.bookFileID : nil)
        guard let fileID else { return nil }
        var start = 0.0
        for file in files {
            if file.bookFileID == fileID {
                return (start + min(max(pos.seconds, 0), file.durationSeconds)) / total
            }
            start += file.durationSeconds
        }
        return nil
    }

    // MARK: Declaration + actions

    private func choice(_ view: AlignmentView) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            SectionLabel("This book has \(view.audioFiles.count) audio files")
            PillSelector(
                options: [CrossFormatLinkMode.sequence, .narrations],
                label: { option in
                    switch option {
                    case .sequence: "One book, in sequence"
                    case .narrations: "Different narrations"
                    }
                },
                selection: $mode
            )
            if mode == .narrations {
                VStack(spacing: 0) {
                    ForEach(Array(view.audioFiles.enumerated()), id: \.element.id) { index, file in
                        RecordRow(label: file.label, isFirst: index == 0) {
                            Button {
                                primary = file.bookFileID
                            } label: {
                                Image(
                                    systemName: primary == file.bookFileID
                                        ? "largecircle.fill.circle" : "circle"
                                )
                                .foregroundStyle(palette.accentColor)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    RecordRule()
                }
            }
        }
    }

    private func matchNote(_ view: AlignmentView) -> some View {
        Group {
            if view.link?.stale == true {
                // Anchoring (and the marks count) is suppressed while
                // stale — say so instead of misreading the zeros.
                Text("The audio files changed since you linked — sync is paused, and the alignment re-checks when you confirm again.")
                    .font(.ui(12))
                    .foregroundStyle(palette.ink2Color)
            } else if let match = view.anchorMatch {
                Text("\u{2713} \(match.matched) of \(match.ebookChapters) chapters matched — jumps land chapter-accurately.")
                    .font(.ui(12))
                    .foregroundStyle(palette.ink2Color)
            } else if (view.audioChapterMarks ?? 0) > 0 {
                Text("The audio's chapter marks couldn't be aligned with this ebook — the mapping is a straight percent-for-percent estimate, so jumps land close, not exact.")
                    .font(.ui(12))
                    .foregroundStyle(palette.ink2Color)
            } else {
                Text("No chapter marks in the audio — the mapping is a straight percent-for-percent estimate, so jumps land close, not exact.")
                    .font(.ui(12))
                    .foregroundStyle(palette.ink2Color)
            }
        }
    }

    /// The al-mobile artboard's legend: each marker named with its value,
    /// so the lanes read without decoding line styles.
    @ViewBuilder
    private func legend(_ view: AlignmentView) -> some View {
        VStack(spacing: 0) {
            if let reading = view.reading, let pct = reading.percent {
                legendRow(dashed: false, label: "You're reading", value: "\(pct)%")
            }
            if let mapped = mappedFraction(view) {
                let total = timelineFiles(view).reduce(0.0) { $0 + $1.durationSeconds }
                if total > 0 {
                    legendRow(
                        dashed: true,
                        label: "Maps to the audio",
                        value: "\u{2248} " + Format.duration(mapped * total)
                    )
                }
            }
            if let listened = listeningFraction(view) {
                let total = timelineFiles(view).reduce(0.0) { $0 + $1.durationSeconds }
                if total > 0 {
                    legendRow(
                        dashed: false,
                        label: "Last listened",
                        value: Format.duration(listened * total)
                    )
                }
            }
        }
        .padding(.top, Spacing.xs)
    }

    private func legendRow(dashed: Bool, label: String, value: String) -> some View {
        HStack(spacing: Spacing.sm) {
            Rectangle()
                .fill(palette.accentColor.opacity(dashed ? 0.7 : 1))
                .frame(width: 2, height: 13)
            Text(label)
                .font(.ui(12))
                .foregroundStyle(palette.ink2Color)
            Spacer()
            Text(value)
                .font(.ui(12, weight: .medium))
                .foregroundStyle(palette.ink1Color)
        }
        .padding(.vertical, 5)
    }

    private func actions(_ view: AlignmentView) -> some View {
        VStack(spacing: Spacing.sm) {
            Button {
                confirm()
            } label: {
                Text(busy ? "Saving…" : "Looks right — sync positions")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(FilledButtonStyle())
            .disabled(busy || !connectivity.isOnline)
            .accessibilityIdentifier("alignment-confirm")

            Button("Not now") { dismiss() }
                .disabled(busy)
                .accessibilityIdentifier("alignment-not-now")

            if view.link != nil {
                Button("Unlink", role: .destructive) { unlink() }
                    .disabled(busy || !connectivity.isOnline)
                    .accessibilityIdentifier("alignment-unlink")
            }
        }
    }

    private func confirm() {
        busy = true
        error = nil
        Task {
            do {
                try await UserDataService.confirmCrossFormatLink(
                    uuid: book.uuid,
                    mode: mode,
                    primaryBookFileID: mode == .narrations ? primary : nil
                )
                busy = false
                onChanged()
                dismiss()
            } catch {
                busy = false
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
    }

    private func unlink() {
        busy = true
        error = nil
        Task {
            do {
                try await UserDataService.unlinkCrossFormat(uuid: book.uuid)
                busy = false
                onChanged()
                dismiss()
            } catch {
                busy = false
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}
