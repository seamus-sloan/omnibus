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

    /// The link waiting on the app becoming ready. Only the newest is kept:
    /// two taps before the app is ready mean the reader changed their mind,
    /// not that they want two books.
    private var pending: DeepLink?
    private var isReady = false

    /// The delivery in flight.
    ///
    /// Deliberately unstructured, and deliberately owned here rather than by a
    /// `.task` on the root view. Resolving a book can take a network read, and
    /// a view-owned task is cancelled the moment anything it observes changes
    /// — including this router. The previous shape cleared an *observed*
    /// `pending` before awaiting, which invalidated the root view, changed the
    /// `.task(id:)` key, and cancelled the delivery mid-flight: the mirror hit
    /// survived (an actor hop ignores cancellation) but the `LibraryService`
    /// fallback did not, so a tap on a book the mirror had not seen — a first
    /// launch, an interrupted sync — silently opened nothing.
    private var delivery: Task<Void, Never>?

    /// Record a link, and act on it if the app is already up.
    func receive(_ url: URL) {
        guard let link = DeepLink(url) else { return }
        pending = link
        deliver()
    }

    /// Told by the root view whenever the auth phase moves. Both halves have to
    /// drive delivery because either can arrive first: a cold launch from a
    /// widget takes the URL then becomes ready, a tap on a running app the
    /// reverse.
    func setReady(_ ready: Bool) {
        isReady = ready
        deliver()
    }

    private func deliver() {
        guard isReady, delivery == nil, let link = pending else { return }
        pending = nil

        delivery = Task { [weak self] in
            switch link {
            case let .book(uuid, format, fileID):
                await self?.open(uuid: uuid, format: format, fileID: fileID)
            }
            self?.delivery = nil
            // A tap that arrived while this one was resolving.
            self?.deliver()
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
