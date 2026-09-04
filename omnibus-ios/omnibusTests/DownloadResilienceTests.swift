//  DownloadResilienceTests.swift
//  The three decisions that made a 1.2 GB audiobook unwinnable on a real link:
//  which drain a queued op belongs to (#2411), whether a stopped transfer is
//  worth resuming (#2410), and whether a download may compete with playback
//  for the uplink (#2409).

import Foundation
import Testing

@testable import omnibus

// MARK: - #2411 · the drain is bounded

struct DrainBoundTests {
    @Test func aPassCoversWhatWasQueuedWhenItStarted() {
        let bound = DrainBound(ceiling: 10)
        #expect(bound.covers(1))
        #expect(bound.covers(10))
    }

    @Test func aPassIgnoresWhatArrivesAfterIt() {
        // The livelock: the player enqueues a coalesced position every 0.5 s,
        // and coalescing re-inserts at a higher rowid. Unbounded, each sweep
        // found the replacement and called it progress forever.
        let bound = DrainBound(ceiling: 10)
        #expect(!bound.covers(11))
        #expect(!bound.covers(9_999))
    }

    /// AC1: with a producer enqueueing during the drain, the pass terminates.
    @Test func aDrainTerminatesAgainstAContinuousProducer() {
        let bound = DrainBound(ceiling: 5)
        var queue: [Int64] = [1, 2, 3, 4, 5]
        var nextID: Int64 = 6
        var sweeps = 0

        while true {
            let inScope = queue.filter(bound.covers)
            guard !inScope.isEmpty else { break }
            sweeps += 1
            // One sweep retires everything in scope; the producer adds another
            // op in the same window, exactly as playback does.
            queue.removeAll { bound.covers($0) }
            queue.append(nextID)
            nextID += 1
            #expect(sweeps < 100, "bounded drain must not livelock")
        }
        #expect(sweeps == 1)
        // AC3: the ops queued during the pass are still there for the next one.
        #expect(queue == [6])
    }

    /// AC3: a later pass, with a higher ceiling, picks up what this one left.
    @Test func aLaterPassTakesWhatThisOneLeftBehind() {
        let first = DrainBound(ceiling: 5)
        let second = DrainBound(ceiling: 6)
        #expect(!first.covers(6))
        #expect(second.covers(6))
    }

    @Test func aCallerJoinsOnlyAPassThatReachesItsOp() {
        let mine = DrainBound(ceiling: 10)
        #expect(mine.satisfied(by: DrainBound(ceiling: 10)))
        #expect(mine.satisfied(by: DrainBound(ceiling: 12)))
        // Joining this one would answer about a pass that stops short of the
        // caller's op — the thing `write` promises not to do.
        #expect(!mine.satisfied(by: DrainBound(ceiling: 9)))
    }
}

// MARK: - #2410 · transient vs terminal

struct DownloadFailureTests {
    @Test func aDroppedLinkIsTransient() {
        for code: URLError.Code in [
            .networkConnectionLost, .timedOut, .notConnectedToInternet,
            .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed,
        ] {
            #expect(DownloadFailure.classify(URLError(code)) == .transient)
        }
    }

    @Test func anUnrecognizedErrorIsTerminal() {
        // The safe default: a download that resumes forever never surfaces an
        // error, so the reader waits on a book that is never coming.
        struct Odd: Error {}
        #expect(DownloadFailure.classify(Odd()) == .terminal)
        #expect(DownloadFailure.classify(URLError(.badURL)) == .terminal)
        #expect(DownloadFailure.classify(URLError(.userAuthenticationRequired)) == .terminal)
    }

    @Test func aServerHavingABadMomentIsTransient() {
        #expect(DownloadFailure.classify(status: 500) == .transient)
        #expect(DownloadFailure.classify(status: 503) == .transient)
        #expect(DownloadFailure.classify(status: 429) == .transient)
        #expect(DownloadFailure.classify(status: 408) == .transient)
    }

    @Test func aDefiniteRefusalIsTerminal() {
        #expect(DownloadFailure.classify(status: 404) == .terminal)
        #expect(DownloadFailure.classify(status: 403) == .terminal)
    }

    /// The gap Copilot caught on #2424: `classify(status:)` was written and
    /// tested but never called, so `finish` abandoned every part of a book on
    /// any non-2xx — including the 5xx a self-hosted server throws while it
    /// restarts. These pin the statuses that must survive as resumable.
    @Test func aRestartingServerDoesNotCostTheReaderTheWholeBook() {
        for status in [500, 502, 503, 504, 408, 429] {
            #expect(
                DownloadFailure.classify(status: status) == .transient,
                "status \(status) must park the part, not abandon the record"
            )
        }
    }

    @Test func aSuccessIsNotAFailure() {
        #expect(DownloadFailure.classify(status: 200) == nil)
        #expect(DownloadFailure.classify(status: 206) == nil)
    }

    @Test func aFailureMessageNamesThePart() {
        #expect(
            DownloadManager.describe("The request timed out", ordinal: 3)
                == "Part 3: The request timed out"
        )
        // A single-file book has no part worth naming.
        #expect(DownloadManager.describe("Gone", ordinal: 0) == "Gone")
        #expect(DownloadManager.describe("Gone", ordinal: nil) == "Gone")
    }

    @Test func resumeDataHasItsOwnNameAndIsSweptWithTheRest() {
        let file = DownloadFile(ordinal: 2, urlPath: "/api/x?part=2", name: "part2.mp3")
        #expect(file.resumeName == "resume.part2.mp3")
        // A removal has to reclaim it, or a cancelled download leaves resume
        // blobs behind that nothing will ever consume.
        #expect(file.onDiskNames.contains(file.resumeName))
    }
}

// MARK: - #2409 · downloads must not race the stream

struct DownloadPolicyTests {
    @Test func aDownloadRunsOnAnIdleWiFiLink() {
        #expect(
            DownloadPolicy.mayTransfer(
                isStreaming: false, wifiOnly: true, pathIsExpensive: false
            )
        )
    }

    @Test func playbackStopsEveryTransfer() {
        // Not only the streamed book's own: the contention is the phone's
        // uplink, which every part shares whatever book it belongs to.
        #expect(
            !DownloadPolicy.mayTransfer(
                isStreaming: true, wifiOnly: false, pathIsExpensive: false
            )
        )
    }

    @Test func wifiOnlyStopsAMeteredTransfer() {
        #expect(
            !DownloadPolicy.mayTransfer(
                isStreaming: false, wifiOnly: true, pathIsExpensive: true
            )
        )
    }

    @Test func aReaderWhoOptsOutMayUseCellular() {
        #expect(
            DownloadPolicy.mayTransfer(
                isStreaming: false, wifiOnly: false, pathIsExpensive: true
            )
        )
    }

    @Test func playbackIsNamedAheadOfTheNetworkWhenBothApply() {
        // The reason the reader can act on: pressing pause resumes the
        // download, whereas "waiting for Wi-Fi" invites them to go looking for
        // a network they may not need.
        #expect(
            DownloadPolicy.haltReason(
                isStreaming: true, wifiOnly: true, pathIsExpensive: true
            ) == .pausedForPlayback
        )
        #expect(
            DownloadPolicy.haltReason(
                isStreaming: false, wifiOnly: true, pathIsExpensive: true
            ) == .waitingForWiFi
        )
        #expect(
            DownloadPolicy.haltReason(
                isStreaming: false, wifiOnly: true, pathIsExpensive: false
            ) == nil
        )
    }

    @Test func haltedStatesReadAsStoppedRatherThanProgressing() {
        #expect(DownloadActivity.pausedForPlayback.isHalted)
        #expect(DownloadActivity.waitingForWiFi.isHalted)
        #expect(DownloadActivity.retrying("timed out").isHalted)
        #expect(DownloadActivity.failed("gone").isHalted)
        #expect(!DownloadActivity.running.isHalted)
        #expect(!DownloadActivity.complete.isHalted)
    }

    @Test func everyHaltedStateSaysWhy() {
        // A stalled bar with no line under it is indistinguishable from a
        // broken one, which is how #2409 read to the listener.
        #expect(DownloadActivity.pausedForPlayback.label == "Paused while listening")
        #expect(DownloadActivity.waitingForWiFi.label == "Waiting for Wi-Fi")
        #expect(DownloadActivity.retrying("timed out").label == "Retrying — timed out")
        #expect(DownloadActivity.failed("Part 3: gone").label == "Part 3: gone")
    }
}
