//  ReaderSettingsStore.swift
//  Test access to the one `UserDefaults` key the reader's typography lives in.
//
//  Two suites write it — the persistence tests and the reboot handover tests —
//  and a suite's `.serialized` orders that suite's own tests and nothing else.
//  Suites still run against each other, so the shared key needs a lock.

import Foundation

@testable import omnibus

/// Serializes the tests that read or write `omnibus.readerSettings`.
///
/// Recursive because a nested call is a deadlock rather than an assertion
/// failure, and a helper that wraps itself is an easy mistake to make.
private let readerSettingsLock = NSRecursiveLock()

/// Runs `body` against an empty `omnibus.readerSettings`, then puts back
/// whatever the test host held.
///
/// Held for the whole of `body`, not just the clear: a reader settings test
/// that ran against another's half-written blob would fail on the other's
/// values rather than its own.
///
/// `body` is deliberately **not** `async`. It runs synchronously on the caller's
/// thread, so a holder can never suspend waiting on an actor — which is what
/// would let a `@MainActor` suite blocked on this lock deadlock against a
/// nonisolated one holding it. Keep it that way; a hang here has no assertion
/// message and burns the whole CI job.
///
/// Only tests that touch the key need this. A controller built with explicit
/// settings — `ReaderController(settings:)` — reads nothing and needs no guard.
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
