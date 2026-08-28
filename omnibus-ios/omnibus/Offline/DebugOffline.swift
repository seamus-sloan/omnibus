//  DebugOffline.swift
//  DEBUG-only simulated airplane mode, for the agentic-exploration iOS lane.
//
//  The whole file is inside `#if DEBUG`, so a Release build has no switch to
//  reach — not a hidden one, not a disabled one, none. See
//  `docs/qa/agentic_exploration/ios_lane.md`.

#if DEBUG

import Foundation

/// Simulated loss of the server, flipped from outside the app.
///
/// `simctl` has no per-app network toggle, so the alternatives were a killable
/// proxy in front of the instance or a switch inside the app. A proxy would lie
/// about *which* traffic died — the widget extension, the background download
/// session and the app share a device but not a client — and it cannot be
/// asserted on from a test. This is the honest version: it fails the same
/// `/api/*` client every real transport failure fails, and nothing else.
///
/// **What it does not cover.** `DownloadManager` runs its own background
/// `URLSession`, so a transfer already in flight keeps going. An audiobook
/// download started while this is on still fails (its manifest read goes
/// through `APIClient`), but a single-file ebook fetch does not. Say so rather
/// than reading a completed download as a defect in the switch.
enum DebugOffline {
    /// Persisted, so the state survives the relaunch an agent uses to prove a
    /// queued write is durable — and so it can be read back from outside the
    /// app, out of the container's preferences plist, without a screenshot.
    static let defaultsKey = "omnibus.debug.forcedOffline"

    /// Launch arguments, for a suite that wants a deterministic starting
    /// state rather than whatever the simulator was left holding.
    static let offlineArgument = "--uitest-offline"
    static let onlineArgument = "--uitest-online"

    /// `omnibus://debug/offline?on=1`. The scheme is already registered for
    /// widget deep links (`DeepLink.scheme`), and `simctl openurl` delivers it
    /// to a running app — which is what makes the toggle usable mid-session
    /// instead of only across a relaunch.
    static let urlHost = "debug"
    static let offlinePath = "offline"
    static let onQuery = "on"

    /// Read straight from `UserDefaults` rather than mirrored into each actor
    /// that asks. One source of truth means there is no window in which the
    /// switch is on and a request path has not heard yet, and `UserDefaults`
    /// is both thread-safe and in-memory after the first read.
    static var isForced: Bool { UserDefaults.standard.bool(forKey: defaultsKey) }

    /// Apply a launch argument, if one was passed. Called from `AppState.init`
    /// alongside the other hermetic hooks, before anything reads the flag.
    ///
    /// Neither argument means "leave it alone", not "go online" — a relaunch
    /// is how an agent proves a queued write survives one, and clearing the
    /// switch on the way would drain the queue it was about to inspect.
    /// `defaults` is injectable so a test can exercise this without writing to
    /// the store every other suite in the process shares.
    static func applyLaunchArguments(
        _ arguments: [String] = ProcessInfo.processInfo.arguments,
        defaults: UserDefaults = .standard
    ) {
        if arguments.contains(offlineArgument) {
            write(true, to: defaults)
        } else if arguments.contains(onlineArgument) {
            write(false, to: defaults)
        }
    }

    /// Persist the flag *and* flush it to the container's plist.
    ///
    /// `synchronize()` is deprecated because an app has no reason to care when
    /// its own preferences reach disk. This one does: the flag is read back
    /// from outside the simulator, and without the flush the harness sees
    /// whatever the app last happened to write — which is how a scenario ends
    /// up believing it went offline when it did not.
    private static func write(_ on: Bool, to defaults: UserDefaults) {
        defaults.set(on, forKey: defaultsKey)
        defaults.synchronize()
    }

    /// Whether `url` is the debug switch, and the state it asks for.
    /// `nil` for anything else, so the caller can hand it to the deep-link
    /// router untouched.
    static func requestedState(from url: URL) -> Bool? {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.scheme?.lowercased() == DeepLink.scheme,
              components.host?.lowercased() == urlHost
        else { return nil }
        let segments = components.path.split(separator: "/", omittingEmptySubsequences: true)
        guard segments.count == 1, segments[0].lowercased() == offlinePath else { return nil }
        let raw = (components.queryItems ?? []).first { $0.name == onQuery }?.value
        // A bare `omnibus://debug/offline` means go offline: the switch's
        // whole job is the awkward state, and the shorter URL is the one an
        // agent types under pressure.
        guard let raw else { return true }
        return ["1", "true", "yes", "on"].contains(raw.lowercased())
    }

    /// Handle `url` if it is the switch. Answers whether it was consumed.
    @MainActor
    @discardableResult
    static func handle(_ url: URL) -> Bool {
        guard let wanted = requestedState(from: url) else { return false }
        set(wanted)
        return true
    }

    /// Flip the switch and tell `Connectivity`, which owns everything that
    /// follows from a reachability change — the offline pill, the probe, and
    /// the outbox drain that fires on the way back.
    ///
    /// The default is written *first*: `Connectivity.apply` re-reads the flag
    /// to clamp a stray path update, so applying in the other order would have
    /// it clamp the very transition being asked for.
    @MainActor
    static func set(_ on: Bool) {
        write(on, to: .standard)
        Connectivity.shared.applyForcedOffline(on)
    }

    /// Re-assert a persisted switch onto a freshly-launched app.
    ///
    /// `Connectivity` seeds itself from the path monitor, which knows nothing
    /// about this, so without a nudge a relaunch in simulated airplane mode
    /// shows itself as online while every request fails.
    @MainActor
    static func restore() {
        guard isForced else { return }
        Connectivity.shared.applyForcedOffline(true)
    }
}

#endif
