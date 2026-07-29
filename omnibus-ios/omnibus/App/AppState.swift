//  AppState.swift
//  Root observable: which server we point at, who is signed in, and the theme.

import Foundation
import Observation
import SwiftUI

@Observable
@MainActor
final class AppState {
    enum Phase: Equatable {
        case launching
        case needsServer
        case needsLogin
        case ready
    }

    private(set) var phase: Phase = .launching
    private(set) var user: UserSummary?
    var serverURL: String?
    var theme: ThemeName {
        didSet { UserDefaults.standard.set(theme.rawValue, forKey: Self.themeKey) }
    }

    /// Server release tag from `/api/_health`, shown alongside the app build
    /// on the You tab.
    private(set) var serverVersion: String?

    private static let themeKey = "omnibus.theme"

    init() {
        // Hermetic hook for omnibusUITests: the suite launches with this
        // argument so a simulator that already holds a server + session still
        // boots to the connect phase. Must run before the loads below.
        if ProcessInfo.processInfo.arguments.contains("--uitest-reset") {
            ServerURLStore.clear()
            TokenStore.clear()
        }
        serverURL = ServerURLStore.load()
        theme = UserDefaults.standard.string(forKey: Self.themeKey)
            .flatMap(ThemeName.init(rawValue:)) ?? .atrium
    }

    var palette: Palette { Palette.named(theme) }

    /// The running build, marked when it's a debug one.
    ///
    /// iOS records nothing about the configuration a build came from, so an
    /// installed app is otherwise indistinguishable from a release one — which
    /// matters on a sideloaded device, where a debug build is unoptimized and
    /// easy to leave on the phone for weeks by accident. Release stays clean.
    var appVersion: String {
        let short = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
        #if DEBUG
        return "\(short) (\(build)) · Debug"
        #else
        return "\(short) (\(build))"
        #endif
    }

    // MARK: - Boot

    func bootstrap() async {
        await OfflineStore.shared.open()
        await DownloadManager.shared.hydrate()
        await Connectivity.shared.refreshPendingCount()

        NotificationCenter.default.addObserver(
            forName: .omnibusUnauthorized, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.handleUnauthorized() }
        }

        guard let serverURL else {
            phase = .needsServer
            return
        }
        await APIClient.shared.configure(baseURL: serverURL)

        guard await APIClient.shared.hasToken() else {
            phase = .needsLogin
            return
        }

        // Paint from the last known identity and confirm behind it. Awaiting
        // the server here is what held the launch screen for a full request
        // timeout every time the app started somewhere without a route to it.
        if let cached = await AuthService.localMe() {
            user = cached
            phase = .ready
        }

        Task { await self.confirmIdentity() }
        Task { await self.refreshServerVersion() }
        Task { await SyncEngine.shared.drain() }
        Task { await LibraryIndex.shared.sync() }
    }

    /// Verify the painted identity against the server. Only an outright
    /// rejection sends the user back to the login screen — an unreachable
    /// server leaves a cached identity signed in, which is the whole point of
    /// holding one.
    private func confirmIdentity() async {
        do {
            user = try await AuthService.me()
            phase = .ready
        } catch APIError.unauthorized {
            phase = .needsLogin
        } catch {
            if user == nil { phase = .needsLogin }
        }
    }

    private func refreshServerVersion() async {
        guard let serverURL else { return }
        if case let .success(version) = await AuthService.probe(serverURL: serverURL) {
            serverVersion = version
        }
    }

    // MARK: - Transitions

    func setServer(_ url: String) async {
        serverURL = url
        ServerURLStore.save(url)
        await APIClient.shared.configure(baseURL: url)

        // Pointing at a server with a token already in the keychain — a
        // reinstall, or re-entering an address after "use a different server" —
        // lands on `.ready` without passing through `signedIn`. That used to
        // skip everything the other two entrances do: the identity was never
        // confirmed, the outbox never drained, and the library was never
        // mirrored, so the app ran indefinitely with an empty mirror and no
        // offline search until something else happened to trigger a sync.
        let authenticated = await APIClient.shared.hasToken()
        phase = authenticated ? .ready : .needsLogin
        await refreshServerVersion()

        guard authenticated else { return }
        Task { await self.confirmIdentity() }
        Task { await SyncEngine.shared.drain() }
        Task { await LibraryIndex.shared.sync(force: true) }
    }

    func changeServer() async {
        await AuthService.logout()
        user = nil
        // A different server is a different library. Nothing this device holds
        // describes the new one: not the mirror, not the cached pages, not the
        // downloads — whose files came from somewhere else entirely. The
        // account-switch wipe can't cover this, since it keys on the username
        // and a username says nothing about which server issued it.
        await OfflineStore.shared.resetForServerChange()
        await DownloadManager.shared.hydrate()
        await Connectivity.shared.refreshPendingCount()
        ServerURLStore.clear()
        serverURL = nil
        await APIClient.shared.configure(baseURL: nil)
        phase = .needsServer
    }

    func signedIn(_ user: UserSummary) {
        self.user = user
        phase = .ready
        Task { await SyncEngine.shared.drain() }
        // A fresh sign-in has no mirror yet, and every offline surface depends
        // on one — pull it now rather than on the first foreground.
        Task { await LibraryIndex.shared.sync(force: true) }
    }

    /// Sign out, pushing anything still queued first.
    ///
    /// After the token is revoked every queued op 401s, and the drain stops on
    /// the first one — so a write made moments before signing out sat on the
    /// device until the same user happened to sign back in, and was wiped
    /// outright if anyone else did.
    func signOut() async {
        await SyncEngine.shared.drain()
        await AuthService.logout()
        await Connectivity.shared.refreshPendingCount()
        user = nil
        phase = .needsLogin
    }

    private func handleUnauthorized() {
        guard phase == .ready else { return }
        user = nil
        phase = .needsLogin
    }
}
