//  PlayerDismissDrag.swift
//  The swipe-down-to-minimize gesture's arithmetic, kept out of the view so it
//  can be pinned by the unit suite rather than only by a finger on a simulator.

import CoreGraphics

/// How a drag on the full player's stage resolves.
enum PlayerDismissDrag {
    /// Below this a drag isn't yet this gesture.
    ///
    /// Non-zero on purpose: `PlayerScrubber` claims its whole band with a
    /// `DragGesture(minimumDistance: 0)`, so a dismiss gesture that also began
    /// at zero would race it and turn a seek into a dismissal.
    static let minimumDistance: CGFloat = 12

    /// Travel past which a release minimizes rather than springs back.
    static let dismissDistance: CGFloat = 120

    /// A flick released short of `dismissDistance` still minimizes when it was
    /// heading far enough. `predictedEndTranslation` is UIKit's own projection
    /// of where the finger was going, so this is velocity without integrating
    /// it by hand.
    static let dismissProjection: CGFloat = 320

    /// How far the surface has travelled for a drag of `translation`.
    ///
    /// Upward and mostly-horizontal drags report zero: the player follows the
    /// finger only for the one gesture it answers to, so a sideways swipe
    /// leaves it exactly where it was.
    static func offset(for translation: CGSize) -> CGFloat {
        guard translation.height > 0, translation.height > abs(translation.width) else {
            return 0
        }
        return translation.height
    }

    /// Whether releasing at `translation` should minimize the player.
    static func shouldDismiss(translation: CGSize, predictedEnd: CGSize) -> Bool {
        guard offset(for: translation) > 0 else { return false }
        if translation.height >= dismissDistance { return true }
        // The projection is put through the same direction filter as the drag,
        // so a swipe that starts down and whips sideways is a sideways swipe
        // however it began — the flick allowance can't smuggle one past.
        return offset(for: predictedEnd) >= dismissProjection
    }

    /// How far through the dismissal a travel of `offset` is, 0...1. Drives the
    /// fade, so it clamps rather than running past the threshold.
    static func progress(at offset: CGFloat) -> CGFloat {
        min(1, max(0, offset / dismissDistance))
    }
}
