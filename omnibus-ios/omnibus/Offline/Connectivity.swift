//  Connectivity.swift
//  Network reachability, and the outbox drain it triggers on reconnect.

import Foundation
import Network
import Observation

@Observable
@MainActor
final class Connectivity {
    static let shared = Connectivity()

    private(set) var isOnline = true
    private(set) var pendingWrites = 0

    /// Writes the server refused outright. They are no longer retried, so
    /// nothing else would ever mention them — and the replica still shows the
    /// change, which makes silence the one answer that misleads.
    private(set) var rejectedWrites = 0

    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "app.omnibus.connectivity")
    private var probe: Task<Void, Never>?
    private var retry: Task<Void, Never>?

    /// How often to re-check a server that stopped answering. Short enough
    /// that coming back feels immediate, long enough to be free on battery.
    private static let probeInterval: Duration = .seconds(10)

    /// How often to push a queue that hasn't emptied. Slower than the probe:
    /// what this covers is a reachable server, so nothing is at risk and there
    /// is no reason to hammer it.
    ///
    /// This is also the cadence at which a *reading position* reaches the
    /// server while someone reads without interruption — the reader and the
    /// player queue their positions rather than pushing each one (see
    /// `SyncEngine.record`), and every event-driven drain fires only when
    /// something actually happens. Sixty seconds is the floor on how stale
    /// another device's view of this one can get, and only in the middle of an
    /// unbroken session.
    private static let pushInterval: Duration = .seconds(60)

    private init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let online = path.status == .satisfied
            Task { @MainActor in
                self?.apply(online: online)
            }
        }
        monitor.start(queue: queue)
        // Seeded from the path the monitor already has rather than left
        // optimistically `true` until the first update lands. Launching in
        // airplane mode otherwise spent that window believing it was online,
        // which is the window the library screen asks its first question in.
        isOnline = monitor.currentPath.status == .satisfied

        NotificationCenter.default.addObserver(
            forName: .omnibusConnectivityChanged, object: nil, queue: .main
        ) { [weak self] note in
            guard let online = note.userInfo?["online"] as? Bool else { return }
            // A transport failure is evidence the *server* is unreachable even
            // when the radio is up, and a completed round trip is evidence it
            // is back — stronger evidence than a path update either way, and
            // the only signal there is when the radio never changed state.
            Task { @MainActor in self?.apply(online: online) }
        }
    }

    private func apply(online: Bool) {
        let wasOffline = !isOnline
        isOnline = online
        Task { await APIClient.shared.setOnline(online) }
        if online {
            probe?.cancel()
            probe = nil
            if wasOffline {
                Task {
                    await SyncEngine.shared.drain()
                    // Coming back is also when the library mirror is most
                    // likely to be out of date — books added while this device
                    // was away are invisible to every offline surface until it
                    // catches up.
                    await LibraryIndex.shared.sync()
                }
            }
        } else {
            // The probe owns getting back on; a retry loop against a server
            // that isn't answering at all is just a second timer doing the
            // first one's job.
            stopRetrying()
            startProbing()
        }
    }

    /// Poll the health endpoint until the server answers again.
    ///
    /// Nothing else will notice on its own: a server that comes back without
    /// the radio ever changing state produces no path update, and the app is
    /// serving the replica happily, so it has no reason to make a request. The
    /// app would sit there reading "Offline" with writes queued until the user
    /// happened to open a screen that fetches. This is what closes that loop —
    /// it goes through `AuthService.probe`, which uses its own session and so
    /// isn't held off by the client's own unreachable back-off.
    private func startProbing() {
        guard probe == nil else { return }
        probe = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.probeInterval)
                guard !Task.isCancelled else { return }
                guard let baseURL = await APIClient.shared.currentBaseURL() else { continue }
                if case .success = await AuthService.probe(serverURL: baseURL) {
                    await MainActor.run { self?.apply(online: true) }
                    return
                }
            }
        }
    }

    func refreshPendingCount() async {
        pendingWrites = await OfflineStore.shared.pendingCount()
        rejectedWrites = await OfflineStore.shared.rejectedCount()
        if pendingWrites > 0 && isOnline { startRetrying() } else { stopRetrying() }
    }

    /// Keep pushing a queue that isn't emptying.
    ///
    /// Every other trigger is an event — a reconnect, a foreground, a book
    /// opened, a book closed. None of them fires for the two cases this
    /// covers: a reachable server that answered 5xx, and a reader who has been
    /// in the same book for an hour. In both the radio never changed, so the
    /// path monitor says nothing and the probe never arms; the app stays online
    /// and simply stops trying. The write then sat until the user happened to
    /// background the app — and because the outbox holds off revalidating the
    /// keys it describes, those rows sat with it.
    ///
    /// One delayed push. The drain's own `notePendingChanged` comes back
    /// through `refreshPendingCount` and arms the next one if anything is still
    /// queued, so this is a loop that always has exactly one runner.
    private func startRetrying() {
        guard retry == nil else { return }
        retry = Task { [weak self] in
            try? await Task.sleep(for: Self.pushInterval)
            guard !Task.isCancelled, let self else { return }
            self.retry = nil
            guard self.isOnline, self.pendingWrites > 0 else { return }
            await SyncEngine.shared.drain()
        }
    }

    private func stopRetrying() {
        retry?.cancel()
        retry = nil
    }

    /// Forget the refused writes.
    ///
    /// Discarding doesn't undo the local change — there is no general way to
    /// invert an arbitrary queued op — but it doesn't preserve it either: a
    /// refused op is already excluded from the queue, so the next read of the
    /// affected key takes the server's answer and the change disappears from
    /// view. What this really clears is the count, and with it the app's only
    /// record that the two ever disagreed.
    func discardRejected() async {
        await OfflineStore.shared.discardRejectedOps()
        await refreshPendingCount()
    }

    /// Called by the write paths so the "N changes waiting to sync" pill is
    /// live without polling.
    func notePendingChanged() {
        Task { await refreshPendingCount() }
    }
}
