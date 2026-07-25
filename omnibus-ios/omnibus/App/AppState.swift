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

        // Offline cold start: trust the cached identity rather than blocking
        // the launch on a request that will time out.
        do {
            user = try await AuthService.me()
            phase = .ready
        } catch APIError.unauthorized {
            phase = .needsLogin
        } catch {
            if let cached: UserSummary = await Cache.cachedOnly(CacheKey.me) {
                user = cached
                phase = .ready
            } else {
                phase = .needsLogin
            }
        }

        Task { await self.refreshServerVersion() }
        Task { await SyncEngine.shared.drain() }
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
        phase = await APIClient.shared.hasToken() ? .ready : .needsLogin
        await refreshServerVersion()
    }

    func changeServer() async {
        await AuthService.logout()
        user = nil
        ServerURLStore.clear()
        serverURL = nil
        await APIClient.shared.configure(baseURL: nil)
        phase = .needsServer
    }

    func signedIn(_ user: UserSummary) {
        self.user = user
        phase = .ready
        Task { await SyncEngine.shared.drain() }
    }

    func signOut() async {
        await AuthService.logout()
        user = nil
        phase = .needsLogin
    }

    private func handleUnauthorized() {
        guard phase == .ready else { return }
        user = nil
        phase = .needsLogin
    }
}
