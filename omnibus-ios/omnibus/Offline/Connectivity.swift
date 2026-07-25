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

    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "app.omnibus.connectivity")

    private init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let online = path.status == .satisfied
            Task { @MainActor in
                self?.apply(online: online)
            }
        }
        monitor.start(queue: queue)

        NotificationCenter.default.addObserver(
            forName: .omnibusConnectivityChanged, object: nil, queue: .main
        ) { [weak self] note in
            guard let online = note.userInfo?["online"] as? Bool else { return }
            // A transport failure is evidence the *server* is unreachable even
            // when the radio is up — trust it for the offline banner, but let
            // the path monitor be the only thing that clears it.
            if !online {
                Task { @MainActor in self?.isOnline = false }
            }
        }
    }

    private func apply(online: Bool) {
        let wasOffline = !isOnline
        isOnline = online
        Task { await APIClient.shared.setOnline(online) }
        if online && wasOffline {
            Task { await SyncEngine.shared.drain() }
        }
    }

    func refreshPendingCount() async {
        pendingWrites = await OfflineStore.shared.pendingCount()
    }

    /// Called by the write paths so the "N changes waiting to sync" pill is
    /// live without polling.
    func notePendingChanged() {
        Task { await refreshPendingCount() }
    }
}
