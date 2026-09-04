//  PlaybackHealth.swift
//  Whether a loaded item is playing, merely starved, or dead.
//
//  `AudioPlayer` used to infer all three from one unconditional `isPlaying`
//  flag that `play()` set and nothing ever cleared, so an item AVFoundation
//  had permanently failed still rendered as playing — pause glyph, live Now
//  Playing card, no audio — until the app was force-quit (#2408). This is the
//  decision half of the fix, kept free of AVFoundation so it can be tested
//  without a live player; `AudioPlayer` translates the framework's signals
//  into `PlaybackSignal` and acts on the returned `PlaybackHealthAction`.

import Foundation

/// A player signal, normalized away from AVFoundation's four overlapping
/// sources (`AVPlayerItem.status`, `AVPlayer.timeControlStatus`,
/// `AVPlayerItemPlaybackStalled`, `AVPlayerItemFailedToPlayToEndTime`).
enum PlaybackSignal: Equatable {
    /// The item became playable, or is actively rendering audio.
    case playing
    /// The item is alive but has run out of buffered data. It re-arms itself
    /// when data arrives, so this is a display state, not a fault.
    case starved
    /// AVFoundation has given up on the item. Terminal for *that* item — only
    /// a freshly built one can play again.
    case failed
}

/// What the player should do about the item's current health.
enum PlaybackHealthAction: Equatable {
    /// Playing normally; clear any buffering or error state.
    case healthy
    /// Starved but armed. Show a buffering state and leave the item alone —
    /// rebuilding here would throw away a recovery already in progress.
    case buffering
    /// Dead. Rebuild the item at the last known position; this is attempt
    /// `attempt` of the budget.
    case rebuild(attempt: Int)
    /// Dead, and the rebuild budget is spent. Stay paused, keep the error
    /// visible, and leave a working play button for a manual retry.
    case surrender
}

/// Tracks how many times the current book has been rebuilt out of a failure,
/// so a genuinely unreachable file settles into a visible error instead of
/// looping on rebuilds forever.
///
/// The budget resets on the first sign of life, not on a timer: a stream that
/// fails once an hour on a flaky link should get a fresh three attempts each
/// time, whereas one that fails three times without ever playing a frame in
/// between is not going to start.
struct PlaybackHealth {
    /// Rebuilds allowed per run of consecutive failures. Three covers the
    /// transient cases (a link that dropped mid-stream, a server restart)
    /// without hammering a file that is genuinely gone.
    static let rebuildBudget = 3

    private(set) var consecutiveFailures = 0

    /// Fold one signal in, and say what the player should do about it.
    mutating func observed(_ signal: PlaybackSignal) -> PlaybackHealthAction {
        switch signal {
        case .playing:
            // Any frame of real audio means the last rebuild worked, so the
            // next failure starts from a full budget.
            consecutiveFailures = 0
            return .healthy
        case .starved:
            // Deliberately does not touch the budget. Starvation is not a
            // failure, but it is not proof of recovery either — an item that
            // stalls, fails, stalls, fails must still exhaust its attempts.
            return .buffering
        case .failed:
            consecutiveFailures += 1
            return consecutiveFailures <= Self.rebuildBudget
                ? .rebuild(attempt: consecutiveFailures)
                : .surrender
        }
    }

    /// Clear the budget for a new book. Called from `teardown()`, so failures
    /// on the book being closed can't spend the next one's attempts.
    mutating func reset() {
        consecutiveFailures = 0
    }

    /// Whether `AudioPlayer.load` may treat the book it already holds as
    /// loaded and return early, rather than tearing down and rebuilding.
    ///
    /// Lives here, and is called by that guard rather than duplicated beside
    /// it, so the rule has one statement and a test can reach it — the whole
    /// bug in #2408 was the early return firing for an item AVFoundation had
    /// already failed, which left `play()` setting a rate on a dead player
    /// with no way back short of a force-quit.
    static func canReuseLoadedItem(
        sameBook: Bool, hasPlayer: Bool, itemFailed: Bool, sameFileRequested: Bool
    ) -> Bool {
        sameBook && hasPlayer && !itemFailed && sameFileRequested
    }
}
