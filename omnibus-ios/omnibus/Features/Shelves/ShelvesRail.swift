//  ShelvesRail.swift
//  Shelves on the landing screen.
//
//  Behind a browse pill, shelves were effectively invisible — you had to know
//  they existed to find them. As a rail under Continue they read as a real part
//  of the library, and the mosaics make each one recognisable at a glance.

import SwiftUI

struct ShelvesRail: View {
    let previews: [ShelfPreview]
    var onSeeAll: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            HStack(alignment: .firstTextBaseline) {
                Text("Shelves")
                    .font(.display(27))
                    .foregroundStyle(palette.ink0Color)

                Spacer(minLength: 0)

                Button(action: onSeeAll) {
                    HStack(spacing: 3) {
                        Text("All")
                        Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                    }
                    .font(.ui(13, weight: .medium))
                    .foregroundStyle(palette.accentColor)
                    // Text alone gives a ~30pt target; pad out to the 44pt
                    // minimum without moving the label.
                    .padding(.vertical, 10)
                    .padding(.leading, 16)
                    .contentShape(Rectangle())
                }
            }
            .screenPadding()

            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 14) {
                    ForEach(Array(previews.enumerated()), id: \.element.id) { index, preview in
                        NavigationLink(value: Destination.shelf(id: preview.shelf.id)) {
                            ShelfCard(preview: preview, width: 108)
                        }
                        .buttonStyle(BookPressStyle())
                        .cascadeIn(index: index)
                        // Cards ease off as they leave, so the rail reads as a
                        // shelf continuing past the edge rather than a list
                        // that stops being drawn.
                        .scrollTransition(axis: .horizontal) { view, phase in
                            view
                                .opacity(phase.isIdentity ? 1 : 0.55)
                                .scaleEffect(phase.isIdentity ? 1 : 0.94)
                        }
                    }

                    NewShelfTile(action: onSeeAll)
                }
                .screenPadding()
                .scrollTargetLayout()
            }
            .scrollIndicators(.hidden)
            .scrollTargetBehavior(.viewAligned)
        }
    }
}

/// Trailing affordance in the rail, so creating a shelf doesn't require
/// finding the index screen first.
private struct NewShelfTile: View {
    var action: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 8) {
                Color.clear
                    .aspectRatio(2.0 / 3.0, contentMode: .fit)
                    .overlay {
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .strokeBorder(
                                palette.line.color,
                                style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                            )
                            .overlay {
                                Image(systemName: "plus")
                                    .font(.system(size: 20, weight: .light))
                                    .foregroundStyle(palette.ink3Color)
                            }
                    }

                Text("New shelf")
                    .font(.ui(13, weight: .medium))
                    .foregroundStyle(palette.ink2Color)
                    .lineLimit(1)
            }
            .frame(width: 108)
        }
        .buttonStyle(PressableStyle())
    }
}
