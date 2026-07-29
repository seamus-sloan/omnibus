//  ConfettiView.swift
//  A one-shot celebratory confetti burst: forty shapes fall once from the top
//  edge, rotating and fading, on independent delays and speeds. Driven by the
//  same explicit spring/curve animations as the rest of the app — each piece
//  is a tiny view animating to "fallen" on appear.

import SwiftUI

/// One piece of confetti. Geometry and timing only — color is an index so the
/// model stays `Equatable` and the view can resolve palette colors itself.
struct ConfettiPiece: Equatable {
    /// Horizontal position as a fraction of the container width.
    var xFraction: Double
    /// Seconds before this piece starts falling.
    var delay: Double
    /// Seconds the fall takes.
    var duration: Double
    /// Base dimension in points; rectangles are `size × size/2`, circles
    /// render at 0.7 scale.
    var size: Double
    var isCircle: Bool
    var colorIndex: Int

    /// Evenly distributed across the width with jitter, staggered starts,
    /// varied speeds and sizes; every 4th piece is a circle.
    static func makePieces(
        count: Int = 40, using rng: inout some RandomNumberGenerator
    ) -> [ConfettiPiece] {
        (0..<count).map { i in
            ConfettiPiece(
                xFraction: min(1, max(0, Double(i) / Double(count) + .random(in: -0.04...0.04, using: &rng))),
                delay: .random(in: 0...0.5, using: &rng),
                duration: .random(in: 1.5...2.6, using: &rng),
                size: .random(in: 5...11, using: &rng),
                isCircle: i % 4 == 0,
                colorIndex: i
            )
        }
    }
}

/// The burst. Fires once on appear; renders nothing under Reduce Motion.
struct ConfettiView: View {
    var accent: Color

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pieces: [ConfettiPiece] = []

    var body: some View {
        if !reduceMotion {
            GeometryReader { geo in
                ZStack(alignment: .topLeading) {
                    ForEach(Array(pieces.enumerated()), id: \.offset) { _, piece in
                        FallingPiece(
                            piece: piece,
                            color: colors[piece.colorIndex % colors.count],
                            bounds: geo.size
                        )
                    }
                }
            }
            .clipped()
            .allowsHitTesting(false)
            .onAppear {
                var rng = SystemRandomNumberGenerator()
                pieces = ConfettiPiece.makePieces(using: &rng)
            }
        }
    }

    /// The book's accent leads; the rest is a fixed festive spread.
    private var colors: [Color] {
        [
            accent,
            Color(red: 0.70, green: 0.62, blue: 0.25),
            Color(red: 0.35, green: 0.55, blue: 0.85),
            Color(red: 0.85, green: 0.72, blue: 0.52),
            Color(red: 0.15, green: 0.55, blue: 0.45),
            Color(red: 0.80, green: 0.42, blue: 0.60),
            .white,
        ]
    }
}

/// One falling shape: starts just above the top edge, lands below the bottom,
/// rotating ~560° and fading out on the way down. The piece starts off-screen,
/// so skipping the design's brief fade-in is invisible in practice.
private struct FallingPiece: View {
    let piece: ConfettiPiece
    let color: Color
    let bounds: CGSize

    @State private var fallen = false

    var body: some View {
        shape
            .rotationEffect(.degrees(fallen ? 560 : 0))
            .opacity(fallen ? 0 : 1)
            .position(
                x: piece.xFraction * bounds.width,
                y: fallen ? bounds.height + piece.size : -piece.size
            )
            .onAppear {
                withAnimation(.easeIn(duration: piece.duration).delay(piece.delay)) {
                    fallen = true
                }
            }
    }

    @ViewBuilder
    private var shape: some View {
        if piece.isCircle {
            Circle()
                .fill(color)
                .frame(width: piece.size * 0.7, height: piece.size * 0.7)
        } else {
            Rectangle()
                .fill(color)
                .frame(width: piece.size, height: piece.size / 2)
        }
    }
}
