//  DownloadProgressView.swift
//  What a download is doing, in enough detail to tell moving from stuck.
//
//  The only bar before this was a bare fraction on the book detail, which says
//  nothing about whether bytes are still arriving. A reader whose 1.2 GB
//  audiobook was starving behind its own streaming playback saw an idle bar and
//  had no way to tell that from a broken one (#2409). Bytes, a state line, and
//  per-part rows for a multi-part book make the difference legible.

import SwiftUI

struct DownloadProgressView: View {
    let record: DownloadRecord
    let activity: DownloadActivity
    /// Per-part rows, for a multi-part audiobook. Off in tight rows.
    var showsParts = true

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            ProgressBar(fraction: record.fraction, tint: tint)

            HStack(spacing: 6) {
                Text(activity.label)
                    .font(.ui(11.5))
                    .foregroundStyle(activity.isHalted ? palette.warnColor : palette.ink2Color)
                    .lineLimit(1)

                if let bytes = byteLine {
                    Text("·").foregroundStyle(palette.ink3Color)
                    Text(bytes)
                        .font(.ui(11.5).monospacedDigit())
                        .foregroundStyle(palette.ink3Color)
                }
            }

            // Only when there is genuinely more than one — a single-file book's
            // "Part 1" row would restate the bar directly above it.
            if showsParts, record.files.count > 1 {
                ForEach(record.files, id: \.ordinal) { file in
                    HStack(spacing: 8) {
                        Text("Part \(file.ordinal)")
                            .font(.ui(10.5))
                            .foregroundStyle(palette.ink3Color)
                            .frame(width: 46, alignment: .leading)
                        ProgressBar(fraction: file.fraction, tint: partTint(file), height: 2)
                        Text(file.done ? "done" : partBytes(file))
                            .font(.ui(10.5).monospacedDigit())
                            .foregroundStyle(palette.ink3Color)
                            .frame(width: 74, alignment: .trailing)
                    }
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityLine)
    }

    private var tint: Color {
        switch activity {
        case .failed: palette.badColor
        case .pausedForPlayback, .waitingForWiFi, .retrying: palette.warnColor
        default: palette.accentColor
        }
    }

    private func partTint(_ file: DownloadFile) -> Color {
        file.done ? palette.okColor : tint
    }

    /// `nil` once complete — the row already prints the finished size, and a
    /// "500 MB of 500 MB" beside it is noise.
    private var byteLine: String? {
        guard activity != .complete, record.totalBytes > 0 else { return nil }
        return "\(Format.bytes(record.receivedBytes)) of \(Format.bytes(record.totalBytes))"
    }

    private func partBytes(_ file: DownloadFile) -> String {
        guard file.totalBytes > 0 else { return Format.bytes(file.receivedBytes) }
        return "\(Int(file.fraction * 100))%"
    }

    private var accessibilityLine: String {
        let percent = Int(record.fraction * 100)
        guard let byteLine else { return "\(activity.label), \(percent) percent" }
        return "\(activity.label), \(percent) percent, \(byteLine)"
    }
}
