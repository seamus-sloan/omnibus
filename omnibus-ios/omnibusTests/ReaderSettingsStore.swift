//  ReaderSettingsStore.swift
//  Test access to the one `UserDefaults` key the reader's typography lives in.
//
//  Two suites write it — the persistence tests and the reboot handover tests —
//  and a suite's `.serialized` orders that suite's own tests and nothing else.
//  Suites still run against each other, so the shared key needs a lock.

import Foundation

@testable import omnibus

/// Serializes every test that touches `omnibus.readerSettings`, across suites.
private let readerSettingsLock = NSLock()

/// Runs `body` against an empty `omnibus.readerSettings`, then puts back
/// whatever the test host held.
///
/// Held for the whole of `body`, not just the clear: a reader settings test
/// that ran against another's half-written blob would fail on the other's
/// values rather than its own.
func withCleanReaderSettings(_ body: () throws -> Void) rethrows {
    readerSettingsLock.lock()
    // Registered first, so it runs last — the store is restored before the next
    // test is let in.
    defer { readerSettingsLock.unlock() }

    let defaults = UserDefaults.standard
    let held = defaults.data(forKey: ReaderSettings.storageKey)
    defaults.removeObject(forKey: ReaderSettings.storageKey)
    defer {
        if let held {
            defaults.set(held, forKey: ReaderSettings.storageKey)
        } else {
            defaults.removeObject(forKey: ReaderSettings.storageKey)
        }
    }
    try body()
}
