//  PlayerScrubber.swift
//  The slim seek track under the artwork.
//
//  `Slider` can't be reshaped this far — it draws a fixed 27pt pill thumb and
//  its own inset chrome — so this is two capsules and a `DragGesture`. The
//  parent owns the drag position so the readouts and the thumb can't disagree;
//  all this keeps locally is whether a finger is down, for the thumb's lift.

import SwiftUI

struct PlayerScrubber: View {
    /// Where playback is, 0...1.
    let fraction: Double
    let tint: Color
    /// The drag position, reported continuously so the readouts can follow it.
    let onScrub: (Double) -> Void
    /// The final position, once the finger lifts.
    let onCommit: (Double) -> Void

    @Environment(\.palette) private var palette

    @State private var isDragging = false

    private let trackHeight: CGFloat = 4
    private let thumbSize: CGFloat = 14

    var body: some View {
        GeometryReader { geo in
            // The thumb travels between its own radii so it never clips an end,
            // and the fill tracks the thumb's centre rather than the raw
            // fraction — otherwise the two disagree by a radius at each end.
            let travel = max(1, geo.size.width - thumbSize)
            let centre = thumbSize / 2 + min(1, max(0, fraction)) * travel

            ZStack(alignment: .leading) {
                Capsule()
                    .fill(palette.ink3Color.opacity(0.35))
                    .frame(height: trackHeight)

                Capsule()
                    .fill(tint)
                    .frame(width: centre, height: trackHeight)

                Circle()
                    .fill(tint)
                    .frame(width: thumbSize, height: thumbSize)
                    .scaleEffect(isDragging ? 1.35 : 1)
                    .offset(x: centre - thumbSize / 2)
            }
            .frame(height: geo.size.height)
            // The whole band takes the touch, not just the 4pt track.
            .contentShape(Rectangle())
            .gesture(
                // `minimumDistance: 0` so a plain tap on the track seeks there
                // too, which is what every player of this shape does.
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        isDragging = true
                        onScrub(position(of: value.location.x, travel: travel))
                    }
                    .onEnded { value in
                        isDragging = false
                        onCommit(position(of: value.location.x, travel: travel))
                    }
            )
            .animation(Motion.snap, value: isDragging)
        }
        .frame(height: 28)
    }

    private func position(of x: CGFloat, travel: CGFloat) -> Double {
        min(1, max(0, (x - thumbSize / 2) / travel))
    }
}
