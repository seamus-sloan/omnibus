//  ReadStatusAuto.swift
//  Automatic read-status transitions driven by the readers: opening an
//  unread book marks it reading, and reaching the end marks it finished —
//  the reader-side sibling of the audio player's finish-on-playback-end,
//  and the native port of the web readers' `read_status_auto`.

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
    func bookOpened() async {
        status = await fetch()
        await applyIfNeeded()
    }

    /// Tell the tracker whether the reader is at the book's end position.
    /// Cheap on every relocate or page turn; an end observed before the fetch
    /// lands is applied when it does.
    func positionChanged(atEnd: Bool) async {
        guard atEnd != self.atEnd else { return }
        self.atEnd = atEnd
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
