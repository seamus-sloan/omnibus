//  TimePatternCharts.swift
//  The Stats tab's "When you read" section: activity by local hour of day and
//  by local weekday.
//
//  Both strips render exactly the buckets the server sent — hours 0...23 and
//  Monday...Sunday, zeros included — and nothing here derives a bucket from a
//  timestamp. The local-time question is settled server-side (`db::stats::
//  patterns`, off the UTC offset each session recorded at capture time)
//  precisely so this screen and the web strips cannot disagree about the same
//  payload, and so a phone carried abroad doesn't relabel last month.

import Charts
import SwiftUI

struct TimePatternCharts: View {
    let summary: StatsSummary

    @Environment(\.palette) private var palette

    /// Hours labelled on the 24-column axis. Labelling all 24 smears them
    /// together at a phone's width; the unlabelled columns still read against
    /// the ticks either side.
    private static let hourTicks = [0, 6, 12, 18]

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            if summary.hasTimePatterns {
                strip("Hour of day") { hourChart }
                // `hasTimePatterns` keys on the hour strip alone, and
                // `chartXScale` takes the weekday domain straight from the
                // wire — so a payload carrying hours without weekdays would
                // hand Charts an empty categorical domain.
                if !summary.dayOfWeek.isEmpty {
                    strip("Day of week") { weekdayChart }
                }
            } else {
                // Both strips are fixed-width, so drawing them here would show
                // a measured-looking day made entirely of zeros.
                Text("No activity with a recorded local time in this period yet.")
                    .font(.ui(13))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize(horizontal: false, vertical: true)
            }

            // Stated rather than absorbed: bucketing sessions that carry no
            // capture-time zone as UTC would put a reader's evening at 4am.
            if summary.unzonedSeconds > 0 {
                Text(
                    "\(Format.humanDuration(summary.unzonedSeconds)) of activity was recorded "
                        + "without a timezone and isn’t shown here."
                )
                .font(.ui(12))
                .foregroundStyle(palette.ink3Color)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var hourChart: some View {
        Chart(summary.hourOfDay) { bucket in
            BarMark(
                x: .value("Hour", Int(bucket.hour)),
                y: .value("Minutes", Double(bucket.seconds) / 60)
            )
            .foregroundStyle(palette.accentColor)
            .cornerRadius(2)
            // There is no hover here to carry the magnitude the way the web
            // strip's title does, so VoiceOver is where the number lives.
            .accessibilityLabel(bucket.clockLabel)
            .accessibilityValue(Format.humanDuration(bucket.seconds))
        }
        // Half a step of padding either side, so the 00 and 23 bars sit inside
        // the plot rather than half-clipped against its edges.
        .chartXScale(domain: -0.5...23.5)
        .chartXAxis {
            AxisMarks(values: Self.hourTicks) { value in
                AxisValueLabel {
                    if let hour = value.as(Int.self) {
                        Text(String(format: "%02d", hour)).font(.monoUI(9))
                    }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading) { _ in
                AxisGridLine().foregroundStyle(palette.line2.color)
                AxisValueLabel().font(.monoUI(9))
            }
        }
        .frame(height: 118)
    }

    private var weekdayChart: some View {
        Chart(summary.dayOfWeek) { bucket in
            BarMark(
                x: .value("Day", bucket.label),
                y: .value("Minutes", Double(bucket.seconds) / 60)
            )
            .foregroundStyle(palette.accentColor)
            .cornerRadius(3)
            .accessibilityLabel(bucket.label)
            .accessibilityValue(Format.humanDuration(bucket.seconds))
        }
        // Pinned to the order the server sent rather than left to Charts'
        // first-seen ordering: week-start is a convention, and the labels are
        // the server's precisely so nothing here decides it.
        .chartXScale(domain: summary.dayOfWeek.map(\.label))
        .chartXAxis {
            AxisMarks { value in
                AxisValueLabel {
                    if let label = value.as(String.self) {
                        Text(label).font(.monoUI(9))
                    }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading) { _ in
                AxisGridLine().foregroundStyle(palette.line2.color)
                AxisValueLabel().font(.monoUI(9))
            }
        }
        .frame(height: 118)
    }

    /// One captioned chart. The section header names both strips together, so
    /// each needs its own quiet line saying which axis it runs on.
    private func strip<Content: View>(
        _ caption: String, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(caption)
                .font(.monoUI(10))
                .foregroundStyle(palette.ink3Color)
            content()
        }
    }
}
