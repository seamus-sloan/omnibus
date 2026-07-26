//  AuthService.swift
//  Bearer login / register / logout, and the `me` lookup.
//
//  Port of `frontend/src/data/auth.rs`. `client_kind: "ios"` is the signal the
//  server uses to issue a bearer token in the JSON body instead of a cookie.

import Foundation
import UIKit

enum AuthService {
    static var deviceName: String {
        get async { await MainActor.run { UIDevice.current.name } }
    }

    static var appVersion: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
    }

    static func login(username: String, password: String) async throws -> UserSummary {
        let request = LoginRequest(
            username: username,
            password: password,
            deviceName: await deviceName,
            clientVersion: appVersion
        )
        let response: LoginResponse = try await APIClient.shared.post("/api/auth/login", body: request)
        return try await finish(response)
    }

    static func register(username: String, password: String) async throws -> UserSummary {
        let request = LoginRequest(
            username: username,
            password: password,
            deviceName: await deviceName,
            clientVersion: appVersion
        )
        let response: LoginResponse = try await APIClient.shared.post("/api/auth/register", body: request)
        return try await finish(response)
    }

    /// Fail loudly when no token came back — a silent fall-through would leave
    /// the app in a half-authenticated state where every later call 401s.
    private static func finish(_ response: LoginResponse) async throws -> UserSummary {
        guard let token = response.token else {
            throw APIError.http(status: 500, message: "Server did not issue a bearer token.")
        }
        await APIClient.shared.setToken(token)
        await OfflineStore.shared.noteUser(response.user.username)
        await Cache.write(CacheKey.me, response.user)
        return response.user
    }

    /// The signed-in identity, settled against the server when it is reachable.
    ///
    /// The account-switch wipe still keys on this answer, but it no longer has
    /// to be network-*first* to be safe: `finish` already runs `noteUser` and
    /// rewrites `CacheKey.me` inside the login itself, so a replica can only
    /// ever hold the identity that the current token belongs to. A 401 still
    /// propagates rather than resolving to the cached user — `Cache.live`
    /// absorbs offline failures only.
    static func me() async throws -> UserSummary {
        let user: UserSummary = try await Cache.settled(CacheKey.me) {
            try await APIClient.shared.get("/api/auth/me")
        }
        await OfflineStore.shared.noteUser(user.username)
        return user
    }

    /// The last known identity, with no network in the path — what the launch
    /// paints before `me()` confirms it.
    static func localMe() async -> UserSummary? {
        await Cache.cachedOnly(CacheKey.me)
    }

    static func logout() async {
        // Best-effort revoke, then always clear locally so a network failure
        // can't wedge the device in a signed-in state.
        let _: Empty? = try? await APIClient.shared.post("/api/auth/logout", body: Empty())
        await APIClient.shared.setToken(nil)
        await OfflineStore.shared.cacheDelete(CacheKey.me)
    }

    /// Probe a server URL before saving it, so the Connect screen can tell
    /// "wrong address" from "wrong password".
    static func probe(serverURL: String) async -> Result<String?, APIError> {
        guard let url = URL(string: serverURL + "/api/_health") else {
            return .failure(.notConfigured)
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = 8
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                return .failure(.http(status: (response as? HTTPURLResponse)?.statusCode ?? 0, message: ""))
            }
            let health = try? JSONDecoder().decode(HealthResponse.self, from: data)
            return .success(health?.version)
        } catch {
            return .failure(.transport(error.localizedDescription))
        }
    }

    static func setKindleEmail(_ email: String?) async throws {
        struct Body: Encodable { let email: String? }
        let _: Empty = try await APIClient.shared.post("/api/account/kindle-email", body: Body(email: email))
        await OfflineStore.shared.cacheDelete(CacheKey.me)
    }
}
