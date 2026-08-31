//  StatsDistributions.swift
//  The windowed drill-ins the redesign's tiles summarise: how the window's
//  books were rated, how long they were, who and what carried the hours, and
//  the covers themselves.
//
//  All of them are scoped by `StatsRange`, which is why they live inside the
//  "In this window" band rather than under the standing rule.

import Charts
import SwiftUI

/// How the window's ratings fell across the ten half-star buckets — the shape
/// the Avg rating tile flattens into one number.
///
/// Only when something was actually rated: ten flat bars describe an empty
/// window less honestly than no chart does.
struct RatingDistribution: View {
    let buckets: [RatingBucket]

    @Environment(\.palette) private var palette

    var body: some View {
        if buckets.contains(where: { $0.books > 0 }) {
            StatsSection("How you rated them") {
                Chart(buckets) { bucket in
                    BarMark(
                        x: .value("Rating", bucket.starLabel),
                        y: .value("Books", bucket.books)
                    )
                    .foregroundStyle(palette.accentColor)
                    .cornerRadius(3)
                }
                .chartXAxis {
                    AxisMarks { value in
                        AxisValueLabel {
                            // Whole stars only: ten labels crowd illegibly at
                            // a phone's width, and the half-star bars sit
                            // between the ones kept.
                            if let label = value.as(String.self), !label.contains(".") {
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
                .frame(height: 132)
            }
        }
    }
}

/// Books finished in the window by length.
///
/// Same rule as the rating chart: nothing finished in the window is an absent
/// chart, not a row of flat bars. The Unknown bucket is rendered whenever it
/// has books in it — an audiobook has no page count, and hiding that would
/// report the distribution over fewer books than were finished.
struct LengthDistribution: View {
    let buckets: [LengthBucket]

    @Environment(\.palette) private var palette

    var body: some View {
        if buckets.contains(where: { $0.books > 0 }) {
            StatsSection("How long they were") {
                Chart(buckets) { bucket in
                    BarMark(
                        x: .value("Books", bucket.books),
                        y: .value("Length", bucket.label)
                    )
                    .foregroundStyle(palette.accentColor)
                    .cornerRadius(3)
                }
                // Horizontal: the labels are page ranges, which don't fit
                // under a column but read fine beside a bar.
                .chartXAxis {
                    AxisMarks { _ in
                        AxisGridLine().foregroundStyle(palette.line2.color)
                        AxisValueLabel().font(.monoUI(9))
                    }
                }
                .chartYAxis {
                    AxisMarks(position: .leading) { _ in
                        AxisValueLabel().font(.monoUI(9))
                    }
                }
                .frame(height: 132)
            }
        }
    }
}

/// A ranked strip — top authors, top tags — where every row leads somewhere.
/// A ranking you can't act on is a picture of a ranking.
struct RankedList: View {
    let entries: [RankedEntity]
    let destination: (RankedEntity) -> Destination

    @Environment(\.palette) private var palette

    private var maximum: Int64 {
        max(1, entries.map(\.seconds).max() ?? 1)
    }

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(entries.prefix(6).enumerated()), id: \.element.id) { index, entry in
                NavigationLink(value: destination(entry)) {
                    row(entry, isFirst: index == 0)
                }
                .buttonStyle(PressableStyle())
            }
        }
    }

    private func row(_ entry: RankedEntity, isFirst: Bool) -> some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                Text(entry.name)
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink1Color)
                    .lineLimit(1)
                    .frame(width: 128, alignment: .leading)

                GeometryReader { geometry in
                    ZStack(alignment: .leading) {
                        Capsule()
                            .fill(palette.line2.color)
                            .frame(height: 7)
                        Capsule()
                            .fill(
                                LinearGradient(
                                    colors: [
                                        palette.accentColor.opacity(0.65),
                                        palette.accentColor,
                                    ],
                                    startPoint: .leading,
                                    endPoint: .trailing
                                )
                            )
                            .frame(
                                width: max(
                                    7,
                                    geometry.size.width * CGFloat(entry.seconds)
                                        / CGFloat(maximum)),
                                height: 7
                            )
                    }
                    .frame(maxHeight: .infinity, alignment: .center)
                }
                .frame(height: 14)

                Text(Format.humanDuration(entry.seconds))
                    .font(.monoUI(10.5))
                    .foregroundStyle(palette.ink3Color)
                    .frame(width: 52, alignment: .trailing)
            }
            .padding(.vertical, 9)
            .contentShape(Rectangle())
        }
    }
}

/// The covers finished in the window.
///
/// Titled "Recently finished" rather than with a count: the tile above already
/// states how many, and a second figure that scrolls off after six covers
/// would look like it contradicted it.
struct FinishedRail: View {
    let books: [FinishedBook]

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            StatsSectionLabel("Recently finished")
                .screenPadding()

            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 14) {
                    ForEach(books) { finished in
                        NavigationLink(value: Destination.book(uuid: finished.bookUUID)) {
                            VStack(alignment: .leading, spacing: 7) {
                                BookCover(
                                    identity: CoverIdentity(
                                        uuid: finished.bookUUID,
                                        title: finished.title,
                                        author: finished.author,
                                        hasCover: finished.coverURL != nil
                                    )
                                )
                                .coverShadow()

                                Text(finished.title)
                                    .font(.ui(12, weight: .medium))
                                    .foregroundStyle(palette.ink0Color)
                                    .lineLimit(2)
                                    .multilineTextAlignment(.leading)

                                if let rating = finished.rating {
                                    StarRating(stars: rating, size: 9)
                                }
                            }
                            .frame(width: 88)
                        }
                        .buttonStyle(BookPressStyle())
                    }
                }
                .screenPadding()
                .scrollTargetLayout()
            }
            .scrollIndicators(.hidden)
            .scrollTargetBehavior(.viewAligned)
        }
    }
}
