//  PositionPushThrottle.swift
//  When a relocate is also worth a network round trip.
//
//  The reader, the comic pager, and the audio player all write every relocate
//  into the replica and the outbox unconditionally — that write is a local
//  SQLite put, cheap regardless of how often it happens, and it is what makes
//  a position survive a force-quit (#1666). This governs only the optional
//  extra step on top of that: whether *this* relocate is also pushed over the
//  network right now, or left to go out with the next scheduled push.

import Foundation

/// A per-book decision of whether the current relocate is worth a round trip
/// of its own. `force` (a close, a backgrounding) always says yes; otherwise
/// at most one push escapes per `interval`, and everything in between still
/// lands in the outbox — coalesced by `SyncEngine`, so a burst collapses to
/// one queued op carrying the newest position.
struct PositionPushThrottle {
    private var lastPush = Date.distantPast
    private var suppressNext: Bool
    private let interval: TimeInterval

    /// `suppressFirst` skips arming the window on the very first decision,
    /// for a caller whose first relocate is a restored position echoed back
    /// at open rather than a real page turn — the epub.js `relocate` that
    /// fires the instant `ReaderController.configure` sets the starting CFI,
    /// and the comic pager's equivalent `page = start` in `prepare`. Left
    /// armed, that echo would consume the window and make the first real
    /// page turn wait out a throttle nobody meant to apply to it. The audio
    /// player has no such echo — its ticks begin only once real playback has
    /// resumed — so it opts out.
    init(interval: TimeInterval, suppressFirst: Bool = true) {
        self.interval = interval
        self.suppressNext = suppressFirst
    }

    /// Whether the caller should also push over the network right now.
    ///
    /// The first decision always clears `suppressNext`, whether or not it was
    /// forced — a forced call (a background/close racing ahead of any
    /// relocate) still means the echo has been "seen" and must not go on
    /// suppressing a later, unrelated non-forced relocate. Only a first
    /// decision that was itself non-forced takes the early-suppress return;
    /// a forced first call falls through to the normal push logic below.
    mutating func shouldPush(force: Bool, now: Date = Date()) -> Bool {
        let wasSuppressed = suppressNext
        suppressNext = false
        if wasSuppressed, !force {
            return false
        }
        guard force || now.timeIntervalSince(lastPush) > interval else { return false }
        lastPush = now
        return true
    }
}
