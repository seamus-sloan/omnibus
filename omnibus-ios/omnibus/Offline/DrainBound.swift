//  DrainBound.swift
//  The op-id ceiling one outbox drain is responsible for.
//
//  `performDrain` swept the *live* queue until a pass found it empty, on the
//  reading that each sweep strictly shrinks it. It does not: the player
//  enqueues a coalesced position every 0.5 s, and coalescing deletes and
//  re-inserts, so on any link slower than that each sweep found a fresh
//  replacement, retired it, and reported progress — a drain that never ended
//  and posted one `/api/progress` per round trip for as long as playback
//  lasted (#2411). Bounding a pass to the ids that existed when it started is
//  what restores the shrinking-set argument, and this is that bound.

import Foundation

/// The highest op id a drain pass will look at.
struct DrainBound: Equatable {
    let ceiling: Int64

    /// Whether this pass is responsible for `opID`.
    ///
    /// Ids are monotonic — a coalescing enqueue deletes the old row and
    /// inserts a new one, which takes a *higher* rowid — so "queued before
    /// this pass began" and "id at or below the ceiling" are the same
    /// statement. That is what makes the bound sound rather than approximate.
    func covers(_ opID: Int64) -> Bool { opID <= ceiling }

    /// Whether a caller wanting everything up to `self` can take `other`'s
    /// answer instead of starting a pass of its own.
    ///
    /// A pass that began before the caller's op was queued is going to stop
    /// short of it, so joining it would report an op as pushed that nothing
    /// ever looked at — which is precisely what `write` promises it does not
    /// do.
    func satisfied(by other: DrainBound) -> Bool { other.ceiling >= ceiling }
}
