//  PositionSync.swift
//  Opening a book on the right page when two devices disagree about it.
//
//  Both the reader and the player face the same question at the same moment:
//  this device remembers a position, the server may hold a further one, and
//  nobody has started reading yet. Shared here because getting it wrong is
//  invisible — the wrong answer is a book that opens somewhere plausible and
//  quietly overwrites where you actually were.

import Foundation

enum PositionSync {
    /// How long an open waits on the server before showing what it already has.
    ///
    /// A bound, not a timeout: the read carries on afterwards and whatever it
    /// finds becomes an offer instead of the opening position. Long enough for
    /// a server on the same network to answer, short enough that an unreachable
    /// one isn't felt — and against one that has gone away the client's own
    /// back-off fails the request immediately, so this is rarely spent at all.
    static let openDeadline: Duration = .milliseconds(1200)

    /// The server's position for a book, when it post-dates `local`.
    ///
    /// Runs the live read to completion whether or not anyone is still waiting
    /// on it, so the replica is written either way — which is most of the point
    /// for a book being taken offline.
    static func newerRemote(
        uuid: String, format: ProgressFormat, than local: ProgressRecord?
    ) async -> ProgressRecord? {
        var newest: ProgressRecord?
        for await record in UserDataService.progress(uuid: uuid, format: format).values() {
            guard let record, carriesPosition(record),
                  record.orderingClock > (local?.orderingClock ?? 0)
            else { continue }
            newest = record
        }
        return newest
    }

    /// The further-along of two records for one book, on the only clock both
    /// can be compared on. Ties keep `a` — the caller puts the record whose
    /// metadata it prefers first.
    static func newest(_ a: ProgressRecord?, _ b: ProgressRecord?) -> ProgressRecord? {
        guard let a else { return b }
        guard let b else { return a }
        return b.orderingClock > a.orderingClock ? b : a
    }

    /// Whether a record actually names a place in the book. The endpoint answers
    /// with a row per `(book, format)`, and a row can exist with the position
    /// field for its format unset — which is not somewhere to send a reader.
    private static func carriesPosition(_ record: ProgressRecord) -> Bool {
        switch record.format {
        case .epub: record.epubCFI?.nilIfBlank != nil
        case .audio: (record.audioPositionSeconds ?? 0) > 0
        }
    }
}

/// `task`'s result if it arrives within `deadline`, otherwise `nil` — leaving
/// the task running rather than cancelling it.
///
/// A task group can't express this: it awaits every child before returning, so
/// the slow branch would still gate the result. Whichever of the two yields
/// first wins and the other's answer is dropped.
///
/// Cancelling would be worse than useless here. The read being waited on
/// unwinds through an outbox drain, and joining a drain is not cancellable —
/// so a cancel on the deadline wouldn't be a deadline at all.
func firstResult<T: Sendable>(
    of task: Task<T?, Never>, within deadline: Duration
) async -> T? {
    let (stream, continuation) = AsyncStream<T?>.makeStream(
        bufferingPolicy: .bufferingNewest(1)
    )
    let waiter = Task { continuation.yield(await task.value) }
    let timer = Task {
        try? await Task.sleep(for: deadline)
        continuation.yield(nil)
    }
    defer {
        waiter.cancel()
        timer.cancel()
    }
    for await first in stream { return first }
    return nil
}
