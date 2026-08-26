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

    #if DEBUG
    /// Launch argument that boots straight to the signed-in shell, offline.
    /// `MainTabView` is otherwise unreachable without a live server, which
    /// would leave the shell's own chrome (the tab bar) untestable.
    static let uiTestShellArgument = "--uitest-shell"
    /// Discard port — refused immediately rather than hanging the launch.
    private static let uiTestShellServer = "http://127.0.0.1:9"
    private static let uiTestShellUser = UserSummary(
        id: 1, username: "uitest", isAdmin: true,
        canUpload: true, canEdit: true, canDownload: true
    )
    #endif

    init() {
        // Hermetic hook for omnibusUITests: the suite launches with this
        // argument so a simulator that already holds a server + session still
        // boots to the connect phase. Must run before the loads below. DEBUG-
        // only so a stray argument in a Release scheme can never wipe state
        // (an XCTest-presence check wouldn't help here — XCUITest drives the
        // app from a separate runner process, so XCTest is never loaded in
        // the app itself).
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--uitest-reset") {
            ServerURLStore.clear()
            TokenStore.clear()
        }
        // Second hermetic hook: seed a server and token so the suite reaches
        // the signed-in shell without one. The address is a closed port, so
        // the confirm-behind request fails fast and `confirmIdentity` leaves
        // the cached identity in place. Pair it with `--uitest-reset` to be
        // independent of whatever the simulator already held.
        if ProcessInfo.processInfo.arguments.contains(Self.uiTestShellArgument) {
            ServerURLStore.save(Self.uiTestShellServer)
            TokenStore.save("uitest")
        }
        #endif
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
        // Every exit path, not just the signed-in one. The scene-phase handler
        // doesn't fire for the launch itself, so this is the only thing that
        // re-derives the Home Screen on a cold start — and the paths that end
        // at the connect or login screen are exactly the ones that must: a
        // session revoked elsewhere otherwise leaves the previous account's
        // titles and cover art on a surface outside the app's sandbox.
        defer { Task { await WidgetSnapshotWriter.shared.refresh() } }

        await OfflineStore.shared.open()

        #if DEBUG
        // The cached identity half of `--uitest-shell`, which has to wait for
        // the store to open. `bootstrap` paints from `localMe()`, so this is
        // what carries the launch to `.ready`.
        if ProcessInfo.processInfo.arguments.contains(Self.uiTestShellArgument) {
            await Cache.write(CacheKey.me, Self.uiTestShellUser)
        }
        #endif
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
    /// Re-read the identity after a profile write, so the You tab's name and
    /// avatar repaint from the server's answer rather than a local guess. A
    /// failure leaves the current user in place — the write already succeeded,
    /// and blanking the header would be a worse lie than a stale name.
    func refreshUser() async {
        if let fresh = try? await AuthService.me() { user = fresh }
    }

    private func confirmIdentity() async {
        do {
            user = try await AuthService.me()
            phase = .ready
        } catch APIError.unauthorized {
            phase = .needsLogin
            // `bootstrap`'s defer has already run by now — it fires when
            // `bootstrap` *returns*, which is before this detached task
            // resolves — so it published the still-signed-in snapshot and
            // would never revisit it. A session revoked elsewhere is the
            // motivating case for refreshing at all; it is also the one that
            // arrives here rather than through a transition.
            await WidgetSnapshotWriter.shared.refresh()
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
        // `forgetAll`, not `hydrate`: the registry merges what it reads onto
        // what it already holds, so re-reading an emptied table would leave
        // the previous server's downloads sitting in memory.
        DownloadManager.shared.forgetAll()
        await Connectivity.shared.refreshPendingCount()
        ServerURLStore.clear()
        serverURL = nil
        await APIClient.shared.configure(baseURL: nil)
        phase = .needsServer
        // The Home Screen is outside the app's sandbox, so a snapshot left
        // behind here keeps showing the previous server's books — titles and
        // cover art — to whoever picks the phone up next.
        await WidgetSnapshotWriter.shared.refresh()
    }

    func signedIn(_ user: UserSummary) {
        self.user = user
        phase = .ready
        Task { await SyncEngine.shared.drain() }
        // A fresh sign-in has no mirror yet, and every offline surface depends
        // on one — pull it now rather than on the first foreground.
        //
        // The widget refresh is chained *behind* the mirror rather than run
        // alongside it: a fresh sign-in has no cached resume points either, so
        // a refresh that wins the race asks `LibraryIndex.isPopulated()` before
        // anything has been written and publishes the "No books yet" card to an
        // account with a full library — where it would sit until the next
        // foreground.
        Task {
            await LibraryIndex.shared.sync(force: true)
            await WidgetSnapshotWriter.shared.refresh()
        }
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
        // Same reason as `changeServer`: the widget survives the sign-out.
        await WidgetSnapshotWriter.shared.refresh()
    }

    private func handleUnauthorized() {
        guard phase == .ready else { return }
        user = nil
        phase = .needsLogin
        // The token was already cleared by the 401 that got us here, so this
        // publishes the signed-out card. Without it the Home Screen keeps the
        // revoked session's titles and cover art until some *other* hook
        // happens to fire.
        Task { await WidgetSnapshotWriter.shared.refresh() }
    }
}
