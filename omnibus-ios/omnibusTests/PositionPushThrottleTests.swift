//  PositionPushThrottleTests.swift
//  The reader, the comic pager, and the audio player all share one throttle
//  for deciding whether a relocate is *also* worth a network round trip — the
//  local write into the replica and the outbox happens on every relocate
//  regardless, in the reader/pager/player code that calls this. These pin the
//  throttle's decisions in isolation: force always pushes, the window holds
//  back everything else, and the restored-position echo at open doesn't spend
//  it before the first real page turn arrives (#1666).

import Foundation
import Testing

@testable import omnibus

@Suite("Position push throttle")
struct PositionPushThrottleTests {
    private let epoch = Date(timeIntervalSince1970: 1_700_000_000)

    @Test("the very first relocate is suppressed so it can't arm the window")
    func firstRelocateDoesNotPush() {
        var throttle = PositionPushThrottle(interval: 4)
        // This stands for the restore relocate epub.js posts the instant
        // `configure` sets the starting CFI — not a page turn, so it must not
        // spend the window the first real one needs.
        #expect(throttle.shouldPush(force: false, now: epoch) == false)
    }

    @Test("the first real relocate after the restore echo pushes immediately")
    func firstRealRelocatePushesRightAway() {
        var throttle = PositionPushThrottle(interval: 4)
        _ = throttle.shouldPush(force: false, now: epoch) // the restore echo
        // Arriving a moment later — nowhere near the four-second window —
        // this must still push, because the echo never armed it.
        #expect(throttle.shouldPush(force: false, now: epoch.addingTimeInterval(0.2)) == true)
    }

    @Test("a rapid burst pushes once and throttles the rest, but every one still belongs in the replica")
    func burstPushesOnceButEveryPositionIsDurable() {
        var throttle = PositionPushThrottle(interval: 4)
        _ = throttle.shouldPush(force: false, now: epoch) // restore echo, consumed

        // Three page turns a few hundred milliseconds apart — well inside the
        // four-second window, the way a reader flipping pages quickly does.
        var replica: [String] = []
        let turns = ["page-1", "page-2", "page-3"]
        var pushed: [Bool] = []
        for (offset, cfi) in turns.enumerated() {
            let now = epoch.addingTimeInterval(0.2 + Double(offset) * 0.3)
            // The caller writes to the replica on every call, unconditionally
            // — `shouldPush`'s answer only decides the extra network push.
            replica.append(cfi)
            pushed.append(throttle.shouldPush(force: false, now: now))
        }

        #expect(pushed == [true, false, false])
        // The bug this fixes: the old guard returned before writing anything
        // at all whenever it wasn't going to push, so a throttled burst left
        // the replica holding whatever the *first* call wrote — or nothing.
        // Every relocate reaching the replica means the newest one wins.
        #expect(replica.last == "page-3")
    }

    @Test("force always pushes, resetting the window")
    func forcePushesRegardlessOfTiming() {
        var throttle = PositionPushThrottle(interval: 4)
        _ = throttle.shouldPush(force: false, now: epoch) // restore echo
        _ = throttle.shouldPush(force: false, now: epoch.addingTimeInterval(0.1)) // pushes, arms window
        // A backgrounding a moment later, well inside the window a page turn
        // would still be throttled by.
        #expect(throttle.shouldPush(force: true, now: epoch.addingTimeInterval(0.2)) == true)
    }

    @Test("a relocate after the window has elapsed pushes again")
    func pushesAgainOnceTheWindowElapses() {
        var throttle = PositionPushThrottle(interval: 4)
        _ = throttle.shouldPush(force: false, now: epoch) // restore echo
        _ = throttle.shouldPush(force: false, now: epoch.addingTimeInterval(0.1)) // pushes
        #expect(
            throttle.shouldPush(force: false, now: epoch.addingTimeInterval(4.2)) == true
        )
    }

    @Test("opting out of suppression pushes the very first relocate")
    func suppressFirstCanBeDisabled() {
        // The audio player's ticks begin only once real playback has resumed
        // — there's no separate restored-position echo to guard against.
        var throttle = PositionPushThrottle(interval: 5, suppressFirst: false)
        #expect(throttle.shouldPush(force: false, now: epoch) == true)
    }

    @Test("a forced call before any relocate still clears the suppression")
    func forcedFirstCallDoesNotLeaveSuppressionArmed() {
        // A background/close can race ahead of the restore echo — the reader
        // opens and is immediately backgrounded before epub.js ever posts a
        // relocate. That forced call must still count as "the first decision
        // has been made": a later, unrelated non-forced relocate is governed
        // by the interval alone, not left suppressed forever because the
        // echo it was meant to catch never actually arrived through here.
        var throttle = PositionPushThrottle(interval: 4)
        #expect(throttle.shouldPush(force: true, now: epoch) == true)
        // Long past the window, so this is unambiguously a normal push, not
        // a suppressed one — pinning the regression this test guards.
        #expect(throttle.shouldPush(force: false, now: epoch.addingTimeInterval(10)) == true)
    }
}
