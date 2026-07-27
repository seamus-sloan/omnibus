//  RefreshTaskTests.swift
//  What `refreshTask` promises: a refresh whose caller is cancelled mid-flight
//  still finishes.
//
//  Worth pinning because the failure is silent. SwiftUI cancels a `refreshable`
//  action as soon as the view redraws, every refresh redraws it by publishing
//  the replica, and the cancelled request that followed simply never landed —
//  a pull that looked like it worked and changed nothing.

import Testing

@testable import omnibus

/// Somewhere for a detached action to record that it ran.
@MainActor
private final class Witness {
    var finished = false
    var sawCancellation = true
}

@Suite("Detached refresh work")
@MainActor
struct RefreshTaskTests {
    @Test("action runs to completion after its caller is cancelled")
    func actionSurvivesCallerCancellation() async {
        let witness = Witness()

        let caller = Task { @MainActor in
            await runUncancelled {
                // Long enough that the cancellation below lands mid-action,
                // which is exactly when the refresh used to be dropped.
                try? await Task.sleep(for: .milliseconds(150))
                witness.sawCancellation = Task.isCancelled
                witness.finished = true
            }
        }

        try? await Task.sleep(for: .milliseconds(20))
        caller.cancel()
        await caller.value

        #expect(witness.finished)
        #expect(!witness.sawCancellation)
    }

    @Test("the caller waits for the action rather than returning early")
    func callerWaitsForTheAction() async {
        let witness = Witness()

        await runUncancelled {
            try? await Task.sleep(for: .milliseconds(50))
            witness.finished = true
        }

        #expect(witness.finished)
    }
}
