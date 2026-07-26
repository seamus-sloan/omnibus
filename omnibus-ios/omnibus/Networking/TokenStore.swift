//  TokenStore.swift
//  Bearer-token persistence in the iOS Keychain.
//
//  The Dioxus shell persists the token as a 0o600 file under the app data dir
//  and carries a TODO to harden it (`mobile/src/main.rs`). Keychain with
//  `afterFirstUnlock` is that hardening: encrypted at rest, still readable by
//  a background refresh after a cold boot.

import Foundation
import Security

enum TokenStore {
    private static let service = "app.omnibus.bearer"
    private static let account = "session"

    static func load() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let data = item as? Data,
              let token = String(data: data, encoding: .utf8),
              !token.isEmpty
        else { return nil }
        return token
    }

    static func save(_ token: String) {
        let data = Data(token.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        let status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var insert = query
            insert[kSecValueData as String] = data
            insert[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(insert as CFDictionary, nil)
        }
    }

    static func clear() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)
    }
}

/// Server base URL. Not a secret, so `UserDefaults` is the right home — it
/// also survives a Keychain wipe, which matters because the Connect screen is
/// the only way back in.
enum ServerURLStore {
    private static let key = "omnibus.serverURL"

    static func load() -> String? {
        UserDefaults.standard.string(forKey: key)?.nilIfBlank
    }

    static func save(_ url: String) {
        UserDefaults.standard.set(url, forKey: key)
    }

    static func clear() {
        UserDefaults.standard.removeObject(forKey: key)
    }

    /// Accept what a user actually types — `192.168.1.5:3000`, a trailing
    /// slash, a stray space — and produce something `URL` can resolve.
    static func normalize(_ raw: String) -> String? {
        var s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty else { return nil }
        if !s.contains("://") { s = "http://" + s }
        while s.hasSuffix("/") { s.removeLast() }
        guard let url = URL(string: s), url.host != nil else { return nil }
        return s
    }
}
