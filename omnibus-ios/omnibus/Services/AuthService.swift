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

    /// Replace the caller's hidden-formats list (the formats their library
    /// All Books view excludes). Account configuration — never queued in the
    /// outbox; an offline call fails loudly (rule 08).
    static func setHiddenFormats(_ formats: [String]) async throws {
        struct Body: Encodable { let formats: [String] }
        let _: Empty = try await APIClient.shared.post("/api/account/hidden-formats", body: Body(formats: formats))
        await OfflineStore.shared.cacheDelete(CacheKey.me)
    }

    // MARK: - Profile
    //
    // A profile is account configuration, so these never go through
    // `SyncEngine` — a deferred replay would apply a name the reader has since
    // changed. They throw on failure and the caller surfaces it (rule 08).

    /// Set (or clear, with `nil`) the caller's display name.
    static func setDisplayName(_ name: String?) async throws {
        struct Body: Encodable { let display_name: String? }
        let _: Empty = try await APIClient.shared.post(
            "/api/account/profile", body: Body(display_name: name)
        )
        await OfflineStore.shared.cacheDelete(CacheKey.me)
    }

    /// Upload the caller's avatar, replacing any existing one.
    static func uploadAvatar(_ data: Data, userID: Int64) async throws {
        let _: Empty = try await APIClient.shared.upload(
            "/api/account/avatar",
            fileData: data,
            fileName: "avatar.jpg",
            fieldName: "avatar",
            mimeType: "image/jpeg"
        )
        await Self.refreshAvatarCaches(userID: userID)
    }

    /// Remove the caller's avatar, reverting them to a monogram.
    static func deleteAvatar(userID: Int64) async throws {
        let _: Empty = try await APIClient.shared.delete("/api/account/avatar")
        await Self.refreshAvatarCaches(userID: userID)
    }

    /// Drop the stale copies an avatar write invalidates: the identity blob
    /// carrying `has_avatar`, and the cached image itself — whose URL doesn't
    /// change when the bytes do, so without this the old picture stays on
    /// screen until the revalidation window elapses.
    private static func refreshAvatarCaches(userID: Int64) async {
        await OfflineStore.shared.cacheDelete(CacheKey.me)
        await ImageCache.shared.invalidate(UserAvatar.path(for: userID))
    }
}
