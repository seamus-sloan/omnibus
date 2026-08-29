//  ReadStatusAuto.swift
//  Automatic read-status transitions driven by the readers and the audio
//  player: starting an unread book marks it reading, and reaching its end
//  marks it finished. The native port of the web `read_status_auto`.

import Foundation

/// One book's automatic transitions, owned by the reader that has it open.
///
/// A decision is never made against a guessed status — writing `reading` over
/// an unfetched `finished` would be a downgrade — so an unknown status keeps
/// every transition inert. It is retried on later observations rather than
/// settled once at open, because the opening fetch is exactly what a book
/// opened offline loses. Writes take the same outbox path as the detail
/// screen's manual chip (`UserDataService.setReadStatus`), so they queue
/// offline like any other content-state write.
@MainActor
final class ReadStatusAuto {
    /// `nil` until a fetch lands — the tracker knows nothing, and every
    /// transition stays inert until it does.
    private var status: ReadStatus?
    private var atEnd = false
    private let fetch: () async -> ReadStatus?
    private let write: (ReadStatus) async -> Void
    private let isOnline: () -> Bool

    /// The production wiring for one book, over `UserDataService`.
    convenience init(uuid: String) {
        self.init(
            fetch: { await UserDataService.storedReadStatus(uuid: uuid) },
            write: { await UserDataService.setReadStatus(uuid: uuid, status: $0) },
            isOnline: { Connectivity.shared.isOnline }
        )
    }

    init(
        fetch: @escaping () async -> ReadStatus?,
        write: @escaping (ReadStatus) async -> Void,
        isOnline: @escaping () -> Bool = { true }
    ) {
        self.fetch = fetch
        self.write = write
        self.isOnline = isOnline
    }

    /// The status to write for a book whose stored state is `current`,
    /// observed either at the reader's end position (`atEnd`) or merely open.
    /// `nil` means no write: opening never downgrades a finished book back to
    /// reading, and re-reaching the end of a finished book is a no-op.
    nonisolated static func transition(current: ReadStatus, atEnd: Bool) -> ReadStatus? {
        if atEnd {
            return current == .finished ? nil : .finished
        }
        return current == .unread ? .reading : nil
    }

    /// Fetch the stored status and apply the open transition. Call once per
    /// open, off the reader's critical path — the fetch may cost a round trip.
    func bookOpened() async {
        await fetchIfUnknown()
        await applyIfNeeded()
    }

    /// Tell the tracker where the reader is. Cheap on every relocate or page
    /// turn; an end observed before the fetch lands is applied when it does.
    ///
    /// An unknown status is retried here rather than latched at open. The
    /// opening fetch is the one every book opened offline loses — the replica
    /// has no row to fall back on for a book nobody has marked — and giving up
    /// on it there is what left a book read cover to cover on a plane with no
    /// status at all (#2289).
    func positionChanged(atEnd: Bool) async {
        self.atEnd = atEnd
        // Reaching the end retries whatever the connection is doing: unlike
        // the open transition it cannot downgrade anything, and dropping it
        // loses the strongest completion signal the app gets. The audio player
        // relies on that — it finishes books it was never asked to open. An
        // ordinary relocate only retries when there is a server to ask, so
        // reading offline costs nothing until the device is back.
        if atEnd || isOnline() {
            await fetchIfUnknown()
        }
        await applyIfNeeded()
    }

    /// Learn the stored status, unless this tracker already knows it.
    ///
    /// A status this tracker already holds is kept rather than refetched: it
    /// is either the one this fetch would return, or the newer one this
    /// tracker just wrote. Overwriting the second with a response that
    /// predates it re-runs a transition that has already happened, and writes
    /// `finished` twice.
    ///
    /// That is why the check is repeated **after** the fetch and not only
    /// before it. `fetch` suspends, and every relocate drives this — so a
    /// second observation can settle the status, and write it, while the
    /// first request is still in flight. Assigning unconditionally on the way
    /// out replays that transition, or drops a settled status back to unknown
    /// when the late answer is a failure. Two concurrent fetches can still
    /// both go out; that costs a redundant GET and nothing else.
    private func fetchIfUnknown() async {
        guard status == nil else { return }
        let fetched = await fetch()
        guard status == nil else { return }
        status = fetched
    }

    private func applyIfNeeded() async {
        guard let current = status,
              let next = Self.transition(current: current, atEnd: atEnd)
        else { return }
        // Optimistic: move the tracked status before the write, so the next
        // observation settles on no-transition instead of repeating a write
        // that is still queued.
        status = next
        await write(next)
    }
}
