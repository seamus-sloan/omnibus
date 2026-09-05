//  PlayerDismissDragTests.swift
//  The full player's swipe-down-to-minimize gesture: which drags it answers to,
//  which it must leave to the scrubber, and where the release threshold sits.

import CoreGraphics
import Testing

@testable import omnibus

@Test func dismissDragFollowsADownwardSwipe() {
    #expect(PlayerDismissDrag.offset(for: CGSize(width: 0, height: 60)) == 60)
    // A little sideways drift is still a downward swipe.
    #expect(PlayerDismissDrag.offset(for: CGSize(width: 18, height: 90)) == 90)
    #expect(PlayerDismissDrag.offset(for: CGSize(width: -18, height: 90)) == 90)
}

@Test func dismissDragIgnoresUpwardAndSidewaysSwipes() {
    // Up does nothing at all — the player has nowhere above to go.
    #expect(PlayerDismissDrag.offset(for: CGSize(width: 0, height: -80)) == 0)
    // Mostly-horizontal, even with real downward travel.
    #expect(PlayerDismissDrag.offset(for: CGSize(width: 140, height: 60)) == 0)
    #expect(PlayerDismissDrag.offset(for: CGSize(width: -140, height: 60)) == 0)
}

@Test func dismissDragSpringsBackShortOfTheThreshold() {
    let short = CGSize(width: 0, height: PlayerDismissDrag.dismissDistance - 1)
    #expect(!PlayerDismissDrag.shouldDismiss(translation: short, predictedEnd: short))
}

@Test func dismissDragMinimizesPastTheThreshold() {
    let far = CGSize(width: 0, height: PlayerDismissDrag.dismissDistance)
    #expect(PlayerDismissDrag.shouldDismiss(translation: far, predictedEnd: far))
}

@Test func dismissDragMinimizesOnAShortFlick() {
    // Released at 40pt but projected well past the threshold: a flick counts.
    #expect(
        PlayerDismissDrag.shouldDismiss(
            translation: CGSize(width: 0, height: 40),
            predictedEnd: CGSize(width: 0, height: PlayerDismissDrag.dismissProjection)
        )
    )
}

@Test func dismissDragRefusesAHorizontalFlick() {
    // A fast sideways swipe projects a long way down and must still be refused.
    #expect(
        !PlayerDismissDrag.shouldDismiss(
            translation: CGSize(width: 300, height: 130),
            predictedEnd: CGSize(width: 600, height: 400)
        )
    )
}

@Test func dismissDragRefusesAFlickThatWhipsSideways() {
    // Starts as a clean downward drag, then the finger leaves sideways. The
    // live translation passes the direction filter and only the projection
    // gives it away, which is the case the filter on `translation` alone missed.
    #expect(
        !PlayerDismissDrag.shouldDismiss(
            translation: CGSize(width: 10, height: 30),
            predictedEnd: CGSize(width: 600, height: 350)
        )
    )
}

@Test func dismissDragRefusesAnUpwardFlick() {
    #expect(
        !PlayerDismissDrag.shouldDismiss(
            translation: CGSize(width: 0, height: -130),
            predictedEnd: CGSize(width: 0, height: -400)
        )
    )
}

@Test func dismissDragProgressClampsToTheThreshold() {
    #expect(PlayerDismissDrag.progress(at: 0) == 0)
    #expect(PlayerDismissDrag.progress(at: PlayerDismissDrag.dismissDistance / 2) == 0.5)
    #expect(PlayerDismissDrag.progress(at: PlayerDismissDrag.dismissDistance) == 1)
    // Past the threshold the fade holds rather than running to nothing.
    #expect(PlayerDismissDrag.progress(at: 600) == 1)
    #expect(PlayerDismissDrag.progress(at: -40) == 0)
}

@Test func dismissDragDeclaresItselfLaterThanTheScrubber() {
    // `PlayerScrubber` claims its band with `minimumDistance: 0`. This gesture
    // must not, or a seek that starts with a downward wobble becomes a
    // dismissal.
    #expect(PlayerDismissDrag.minimumDistance > 0)
}
