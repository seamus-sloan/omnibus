//  PlaybackHealthTests.swift
//  The player's decisions about a failed, starved, or healthy item.
//
//  `AudioPlayer` inferred all three from one `isPlaying` flag that `play()`
//  set and nothing cleared, so a permanently-failed item still rendered as
//  playing until the app was force-quit (#2408). These pin the policy that
//  replaced it: what a failure buys, what starvation costs, when the budget
//  refills, and when a re-open of the loaded book must rebuild rather than
//  return early into a dead player.

import Testing

@testable import omnibus

struct PlaybackHealthTests {
    @Test func failureAsksForARebuildWhileTheBudgetHolds() {
        var health = PlaybackHealth()
        #expect(health.observed(.failed) == .rebuild(attempt: 1))
        #expect(health.observed(.failed) == .rebuild(attempt: 2))
        #expect(health.observed(.failed) == .rebuild(attempt: 3))
    }

    @Test func failureSurrendersOnceTheBudgetIsSpent() {
        var health = PlaybackHealth()
        for _ in 0..<PlaybackHealth.rebuildBudget { _ = health.observed(.failed) }
        // The listener is left with a visible error and a working play button
        // rather than a player that keeps silently rebuilding a file that is
        // never coming back.
        #expect(health.observed(.failed) == .surrender)
        #expect(health.observed(.failed) == .surrender)
    }

    @Test func playingRefillsTheBudget() {
        var health = PlaybackHealth()
        _ = health.observed(.failed)
        _ = health.observed(.failed)
        #expect(health.observed(.playing) == .healthy)
        // A flaky link that drops once an hour gets a full budget each time;
        // only failures with no audio in between are treated as one run.
        #expect(health.observed(.failed) == .rebuild(attempt: 1))
    }

    @Test func starvationBuffersWithoutSpendingTheBudget() {
        var health = PlaybackHealth()
        #expect(health.observed(.starved) == .buffering)
        #expect(health.observed(.starved) == .buffering)
        #expect(health.consecutiveFailures == 0)
    }

    @Test func starvationDoesNotCountAsRecovery() {
        var health = PlaybackHealth()
        _ = health.observed(.failed)
        _ = health.observed(.starved)
        // Stall-fail-stall-fail must still exhaust the budget — starvation is
        // not proof the item recovered, only that it has not given up yet.
        #expect(health.observed(.failed) == .rebuild(attempt: 2))
    }

    @Test func resetClearsTheBudgetForTheNextBook() {
        var health = PlaybackHealth()
        _ = health.observed(.failed)
        _ = health.observed(.failed)
        health.reset()
        #expect(health.consecutiveFailures == 0)
        #expect(health.observed(.failed) == .rebuild(attempt: 1))
    }

    // MARK: - The same-book reload rule

    @Test func loadedBookIsReusedWhenItsItemIsHealthy() {
        #expect(
            PlaybackHealth.canReuseLoadedItem(
                sameBook: true, hasPlayer: true, itemFailed: false, sameFileRequested: true
            )
        )
    }

    @Test func loadedBookIsRebuiltWhenItsItemHasFailed() {
        // The regression #2408 is named for: returning early here called
        // `play()` on a dead player, so only killing the app recovered.
        #expect(
            !PlaybackHealth.canReuseLoadedItem(
                sameBook: true, hasPlayer: true, itemFailed: true, sameFileRequested: true
            )
        )
    }

    @Test func aDifferentBookIsNeverReused() {
        #expect(
            !PlaybackHealth.canReuseLoadedItem(
                sameBook: false, hasPlayer: true, itemFailed: false, sameFileRequested: true
            )
        )
    }

    @Test func anExplicitDifferentFileIsNeverReused() {
        #expect(
            !PlaybackHealth.canReuseLoadedItem(
                sameBook: true, hasPlayer: true, itemFailed: false, sameFileRequested: false
            )
        )
    }

    @Test func nothingIsReusedWithoutAPlayer() {
        #expect(
            !PlaybackHealth.canReuseLoadedItem(
                sameBook: true, hasPlayer: false, itemFailed: false, sameFileRequested: true
            )
        )
    }
}
