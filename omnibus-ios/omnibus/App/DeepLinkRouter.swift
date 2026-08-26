//  DeepLinkRouter.swift
//  Where an `omnibus://` URL lands.
//
//  A widget tap is almost always a cold launch, so the URL arrives long before
//  `bootstrap` has decided whether there is even an account on the device. The
//  link is therefore recorded rather than acted on, and delivered once the app
//  reaches `.ready` — dropping it because the app wasn't ready yet is what
//  makes a widget tap open the app at whatever screen it was last on.

import Foundation
import Observation

@Observable
@MainActor
final class DeepLinkRouter {
    static let shared = DeepLinkRouter()

    /// The link waiting to be acted on. Observed rather than private so the
    /// root view can re-run delivery when either this or the auth phase moves.
    private(set) var pending: DeepLink?

    /// Record a link. Only the newest is kept: two taps before the app is
    /// ready mean the reader changed their mind, not that they want two books.
    func receive(_ url: URL) {
        guard let link = DeepLink(url) else { return }
        pending = link
    }

    /// Act on whatever is waiting. Safe to call at any phase and any number of
    /// times — it clears the link before opening anything, so a second call
    /// racing the first cannot open the same book twice.
    func deliverPending() async {
        guard let link = pending else { return }
        pending = nil

        switch link {
        case let .book(uuid, format, fileID):
            await open(uuid: uuid, format: format, fileID: fileID)
        }
    }

    private func open(uuid: String, format: WidgetFormat?, fileID: Int64?) async {
        guard let book = await resolve(uuid: uuid) else { return }
        switch book.resumeFormat(for: format) {
        case .audio:
            Presentation.shared.openPlayer(book, fileID: fileID)
        case .epub:
            Presentation.shared.openReader(book)
        }
    }

    /// The mirror first: it holds the whole library, answers with no network,
    /// and is where the snapshot's uuid came from in the first place. The live
    /// read is the fallback for a book the mirror has not seen — a first
    /// launch, or an interrupted sync.
    private func resolve(uuid: String) async -> Book? {
        if let local = await LibraryIndex.shared.book(uuid: uuid) { return local }
        for await book in LibraryService.book(uuid: uuid).values() { return book }
        return nil
    }
}
