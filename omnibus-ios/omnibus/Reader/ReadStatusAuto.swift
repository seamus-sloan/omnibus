//  ReadStatusAuto.swift
//  Automatic read-status transitions driven by the readers and the audio
//  player: starting an unread book marks it reading, and reaching its end
//  marks it finished. The native port of the web `read_status_auto`.

import Foundation

/// One book's automatic transitions, owned by the reader that has it open.
///
/// The stored status is fetched once per open, and `nil` — the fetch failed —
/// keeps every transition inert: a decision is never made against a guessed
/// status, because writing `reading` over an unfetched `finished` would be a
/// downgrade. Writes take the same outbox path as the detail screen's manual
/// chip (`UserDataService.setReadStatus`), so they queue offline like any
/// other content-state write.
@MainActor
final class ReadStatusAuto {
    /// `nil` until the fetch lands — and stays `nil` when it fails.
    private var status: ReadStatus?
    private var atEnd = false
    private let fetch: () async -> ReadStatus?
    private let write: (ReadStatus) async -> Void

    /// The production wiring for one book, over `UserDataService`.
    convenience init(uuid: String) {
        self.init(
            fetch: { await UserDataService.storedReadStatus(uuid: uuid) },
            write: { await UserDataService.setReadStatus(uuid: uuid, status: $0) }
        )
    }

    init(
        fetch: @escaping () async -> ReadStatus?,
        write: @escaping (ReadStatus) async -> Void
    ) {
        self.fetch = fetch
        self.write = write
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
    ///
    /// A status this tracker already holds is kept rather than refetched: it
    /// is either the one this fetch would return, or the newer one this
    /// tracker just wrote. Overwriting the second with a response that
    /// predates it re-runs a transition that has already happened, and writes
    /// `finished` twice.
    func bookOpened() async {
        if status == nil {
            status = await fetch()
        }
        await applyIfNeeded()
    }

    /// Tell the tracker whether the reader is at the book's end position.
    /// Cheap on every relocate or page turn; an end observed before the fetch
    /// lands is applied when it does.
    func positionChanged(atEnd: Bool) async {
        guard atEnd != self.atEnd else { return }
        self.atEnd = atEnd
        // Reaching the end is the one observation worth a second round trip
        // when the opening fetch failed or never ran: unlike the open
        // transition it cannot downgrade anything, and dropping it loses the
        // strongest completion signal the app gets. The audio player relies
        // on this — it finishes books it was never asked to open.
        if atEnd, status == nil {
            status = await fetch()
        }
        await applyIfNeeded()
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
