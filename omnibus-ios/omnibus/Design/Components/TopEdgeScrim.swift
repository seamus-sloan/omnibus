//  TopEdgeScrim.swift
//  What the status bar sits on when a screen has no navigation bar.
//
//  The four tab roots hide the bar so the masthead can be the header, which
//  also removes the thing iOS hangs its scroll edge effect on: covers and rows
//  slid out from behind the clock with a hard edge and no separation, which is
//  the single loudest "unfinished app" cue there is. `scrollEdgeEffectStyle`
//  compiles here but draws nothing without a bar to anchor to, so this is that
//  effect by hand — a blur that fades out over the top band, giving the status
//  bar a ground without costing the header band a real bar would reserve.

import SwiftUI

struct TopEdgeScrim: View {
    /// Measured from the top of the screen, not the safe area: tall enough to
    /// clear the Dynamic Island, with room under it for the fade.
    var height: CGFloat = 104

    var body: some View {
        Rectangle()
            .fill(.ultraThinMaterial)
            .mask(
                LinearGradient(
                    stops: [
                        .init(color: .black, location: 0),
                        .init(color: .black, location: 0.45),
                        .init(color: .black.opacity(0.55), location: 0.72),
                        .init(color: .clear, location: 1),
                    ],
                    startPoint: .top,
                    endPoint: .bottom
                )
            )
            .frame(height: height)
            .frame(maxHeight: .infinity, alignment: .top)
            .ignoresSafeArea(edges: .top)
            .allowsHitTesting(false)
    }
}

extension View {
    /// Applies the status-bar scrim over a bar-less scrolling screen.
    func topEdgeScrim(height: CGFloat = 104) -> some View {
        overlay(alignment: .top) { TopEdgeScrim(height: height) }
    }
}
