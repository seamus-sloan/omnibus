//  LifecycleSync.swift
//  What happens when the app comes forward and when it goes away.
//
//  Syncing on a schedule of its own is invisible; syncing on the two moments a
//  reader already thinks of as "leaving" and "coming back" is the whole of what
//  makes an app feel synced. Nothing here is new machinery — it is the outbox,
//  the replica, and the library mirror, fired at the moments that matter.

import BackgroundTasks
import Foundation
import UIKit

@MainActor
final class LifecycleSync {
    static let shared = LifecycleSync()

    /// Must match the entry in `BGTaskSchedulerPermittedIdentifiers`.
    static let backgroundTaskID = "app.omnibus.sync"

    /// The soonest iOS is asked to run a background pass. It is a floor, not a
    /// schedule — the system decides when, based on how the app is actually
    /// used. Asking for something short doesn't make it happen sooner.
    private static let backgroundInterval: TimeInterval = 3600

    /// What an open book does when the app leaves and when it comes back. The
    /// reader and the player register themselves while they're on screen; the
    /// closures are what let backgrounding persist a position, and foregrounding
    /// restart a reading session, without this knowing what a reader or a player
    /// is.
    private struct BookHooks {
        var flush: @MainActor () async -> Void
        var resume: @MainActor () async -> Void
    }

    private var openBooks: [ObjectIdentifier: BookHooks] = [:]

    private var lastForeground: Date?

    /// Ignore a return to the app that follows a departure by only a moment —
    /// a permission sheet, a share sheet, the app switcher glimpsed and
    /// dismissed. Re-syncing on those is all cost and no news.
    private static let foregroundDebounce: TimeInterval = 20

    /// Register an open book for the duration of its time on screen. `flush`
    /// runs on the way to the background, `resume` when the app comes back.
    func register(
        _ owner: AnyObject,
        flush: @escaping @MainActor () async -> Void,
        resume: @escaping @MainActor () async -> Void = {}
    ) {
        openBooks[ObjectIdentifier(owner)] = BookHooks(flush: flush, resume: resume)
    }

    func unregister(_ owner: AnyObject) {
        openBooks.removeValue(forKey: ObjectIdentifier(owner))
    }

    /// The app came forward.
    ///
    /// Drain first: a queued position is the thing most likely to be wrong on
    /// another device, and pushing before pulling is what stops this device's
    /// own pending write from arriving *after* it has already read past it.
    func didEnterForeground() async {
        // Ahead of the debounce, and unconditionally. A reader that stopped
        // counting on the way out has to start counting again on the way back
        // however brief the trip was — skipping it because the round trip was
        // quick would mean the rest of that reading session went unrecorded.
        for hooks in openBooks.values { await hooks.resume() }

        if let lastForeground,
           Date().timeIntervalSince(lastForeground) < Self.foregroundDebounce {
            return
        }
        lastForeground = Date()

        await SyncEngine.shared.drain()
        await Connectivity.shared.refreshPendingCount()

        // The replica entries a returning reader is most likely to be looking
        // at. Everything else refreshes when its screen asks.
        async let mirror: Void = LibraryIndex.shared.sync()
        async let resume: Void = refreshResumePoints()
        _ = await (mirror, resume)

        await WidgetSnapshotWriter.refresh()
    }

    /// A book was opened.
    ///
    /// Kindle treats opening a book as a sync point, and it earns its place:
    /// this is the moment a reader is most likely to have positions queued from
    /// a session elsewhere, and the moment they most care that the position
    /// they're about to be shown is the right one. The drain runs alongside the
    /// reader's own reconcile rather than in front of it — nothing here blocks
    /// the page appearing.
    func didOpenBook() {
        Task { await SyncEngine.shared.drain() }
    }

    /// A book was closed.
    ///
    /// The position and the session report have already been written by the
    /// reader; this pushes them, plus anything still queued behind them, rather
    /// than leaving it all for whenever the app next changes phase.
    func didCloseBook() {
        Task {
            await SyncEngine.shared.drain()
            await Connectivity.shared.refreshPendingCount()
            // The end of a session is the moment the Home Screen is most likely
            // to be wrong — the book just moved to the front of the rail, with
            // a new position under it.
            await WidgetSnapshotWriter.refresh()
        }
    }

    /// The app is going away.
    ///
    /// iOS suspends shortly after this returns, so the work runs inside a
    /// background-task assertion — without one, a position written on the way
    /// out is a race against suspension that the position usually loses.
    func didEnterBackground() async {
        lastForeground = nil
        let application = UIApplication.shared
        var identifier: UIBackgroundTaskIdentifier = .invalid

        let work = Task {
            for hooks in openBooks.values { await hooks.flush() }
            await SyncEngine.shared.drain()
            await Connectivity.shared.refreshPendingCount()
            // After the flush, so the position the reader wrote on its way out
            // is the one the Home Screen shows — this is the pass that makes a
            // widget correct while the app is closed, which is the only state
            // anyone ever looks at one in.
            await WidgetSnapshotWriter.refresh()
            scheduleBackgroundRefresh()
        }

        identifier = application.beginBackgroundTask(withName: "omnibus.sync") {
            // Out of time. Cancelling matters as much as ending the assertion:
            // the work carries on into suspension otherwise, and a drain frozen
            // mid-pass is what left landed writes still queued to be re-sent on
            // the next launch. Ending the assertion is mandatory too — iOS
            // kills the app outright if it expires unended.
            work.cancel()
            if identifier != .invalid {
                application.endBackgroundTask(identifier)
                identifier = .invalid
            }
        }

        await work.value

        if identifier != .invalid {
            application.endBackgroundTask(identifier)
            identifier = .invalid
        }
    }

    /// Pull the Continue-reading rail through its live read so the replica is
    /// current before any screen asks. The values themselves are the caller's
    /// business — this is here for the write into the cache the read performs.
    private func refreshResumePoints() async {
        for await _ in UserDataService.recentProgress().values() {}
    }

    // MARK: - Background refresh

    /// Register the background handler. Must run before the app finishes
    /// launching, or iOS rejects it.
    nonisolated func registerBackgroundTask() {
        BGTaskScheduler.shared.register(
            forTaskWithIdentifier: Self.backgroundTaskID, using: nil
        ) { task in
            guard let refresh = task as? BGAppRefreshTask else {
                task.setTaskCompleted(success: false)
                return
            }
            Task { @MainActor in await LifecycleSync.shared.runBackgroundRefresh(refresh) }
        }
    }

    /// Ask for another pass. Called on the way to the background — a submitted
    /// request is consumed by the run it triggers, so each one has to be
    /// re-armed.
    func scheduleBackgroundRefresh() {
        let request = BGAppRefreshTaskRequest(identifier: Self.backgroundTaskID)
        request.earliestBeginDate = Date(timeIntervalSinceNow: Self.backgroundInterval)
        // Throws on a simulator without the capability, and whenever the user
        // has turned Background App Refresh off. Neither is an error worth
        // surfacing — the app simply syncs when it is next opened, which is
        // where it was before any of this existed.
        try? BGTaskScheduler.shared.submit(request)
    }

    /// One background pass: push what's queued, pull what a returning reader
    /// sees first.
    ///
    /// Deliberately narrow. The system grants seconds, not minutes, and killing
    /// the app for overrunning costs future grants — so this drains the outbox
    /// and refreshes the resume rail, and leaves the library mirror to a real
    /// foreground where there's time to page through it.
    private func runBackgroundRefresh(_ task: BGAppRefreshTask) async {
        // Re-arm first: an early return past this point would otherwise end the
        // chain and the app would never be woken again.
        scheduleBackgroundRefresh()

        let work = Task {
            await SyncEngine.shared.drain()
            await refreshResumePoints()
            await Connectivity.shared.refreshPendingCount()
            // The whole reason a widget can be right about a session that
            // ended on another device: this pass runs without the app ever
            // being opened, and the Home Screen redraws off the back of it.
            await WidgetSnapshotWriter.refresh()
        }
        task.expirationHandler = { work.cancel() }
        await work.value
        task.setTaskCompleted(success: !work.isCancelled)
    }
}
