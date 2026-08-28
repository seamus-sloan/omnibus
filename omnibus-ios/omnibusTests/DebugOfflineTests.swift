//  DebugOfflineTests.swift
//  The DEBUG-only simulated-offline switch: what URLs it claims, what it
//  leaves for the deep-link router, and what a launch argument does to the
//  persisted flag.
//
//  The switch is driven from outside the app by an agent that gets no reply —
//  `simctl openurl` reports that iOS accepted the URL, never that the app
//  understood it — so a parsing mistake here is silent, and the scenario that
//  follows tests nothing while looking like it passed.

import Foundation
import Testing

@testable import omnibus

#if DEBUG

private func url(_ string: String) throws -> URL {
    try #require(URL(string: string))
}

@Suite("Debug offline switch URLs")
struct DebugOfflineURLTests {
    @Test("on=1 asks to go offline")
    func explicitOnGoesOffline() throws {
        #expect(DebugOffline.requestedState(from: try url("omnibus://debug/offline?on=1")) == true)
    }

    @Test("on=0 asks to come back online")
    func explicitOffGoesOnline() throws {
        #expect(DebugOffline.requestedState(from: try url("omnibus://debug/offline?on=0")) == false)
    }

    @Test(
        "the truthy spellings all mean offline",
        arguments: ["1", "true", "TRUE", "yes", "on"]
    )
    func truthySpellings(_ raw: String) throws {
        #expect(
            DebugOffline.requestedState(from: try url("omnibus://debug/offline?on=\(raw)")) == true
        )
    }

    @Test(
        "anything else means online, rather than being refused",
        arguments: ["0", "false", "no", "off", ""]
    )
    func falsySpellings(_ raw: String) throws {
        #expect(
            DebugOffline.requestedState(from: try url("omnibus://debug/offline?on=\(raw)")) == false
        )
    }

    @Test("a bare switch URL means offline")
    func bareURLGoesOffline() throws {
        #expect(DebugOffline.requestedState(from: try url("omnibus://debug/offline")) == true)
    }

    @Test("the host is matched case-insensitively, like the rest of the scheme")
    func hostIsCaseInsensitive() throws {
        #expect(DebugOffline.requestedState(from: try url("OMNIBUS://DEBUG/OFFLINE")) == true)
    }

    @Test("a book deep link is not the switch")
    func bookLinkIsNotTheSwitch() throws {
        let link = try url("omnibus://book/18c784fc-0000-4000-8000-000000000000")
        #expect(DebugOffline.requestedState(from: link) == nil)
    }

    @Test("another scheme is not the switch")
    func foreignSchemeIsNotTheSwitch() throws {
        #expect(DebugOffline.requestedState(from: try url("https://debug/offline?on=1")) == nil)
    }

    @Test(
        "a near-miss path is refused rather than guessed at",
        arguments: [
            "omnibus://debug",
            "omnibus://debug/",
            "omnibus://debug/offline/extra",
            "omnibus://debug/online",
            "omnibus://debugging/offline",
        ]
    )
    func nearMissesAreRefused(_ raw: String) throws {
        #expect(DebugOffline.requestedState(from: try url(raw)) == nil)
    }
}

@Suite("Debug offline switch and the deep-link router")
struct DebugOfflineRoutingTests {
    /// The two share `omnibus://`, and each must ignore the other's URLs —
    /// the switch consuming a book link would swallow a widget tap, and the
    /// router claiming the switch would leave an agent toggling nothing.
    @Test("the router does not parse the switch as a book")
    func routerIgnoresTheSwitch() throws {
        #expect(DeepLink(try url("omnibus://debug/offline?on=1")) == nil)
    }

    @Test("the switch does not claim a book link the router still parses")
    func switchLeavesBookLinksAlone() throws {
        let link = try url("omnibus://book/18c784fc-0000-4000-8000-000000000000?format=epub")
        #expect(DebugOffline.requestedState(from: link) == nil)
        #expect(
            DeepLink(link)
                == .book(uuid: "18c784fc-0000-4000-8000-000000000000", format: .epub, fileID: nil)
        )
    }
}

@Suite("Debug offline launch arguments")
struct DebugOfflineLaunchArgumentTests {
    /// A store of its own: `applyLaunchArguments` writes the same key the live
    /// switch reads, and the unit suite runs in the app's own process.
    private func scratchDefaults() throws -> UserDefaults {
        let suite = "omnibus.tests.debugOffline.\(UUID().uuidString)"
        return try #require(UserDefaults(suiteName: suite))
    }

    @Test("--uitest-offline pins the switch on")
    func offlineArgumentPinsOn() throws {
        let defaults = try scratchDefaults()
        DebugOffline.applyLaunchArguments(["omnibus", "--uitest-offline"], defaults: defaults)
        #expect(defaults.bool(forKey: DebugOffline.defaultsKey))
    }

    @Test("--uitest-online pins the switch off")
    func onlineArgumentPinsOff() throws {
        let defaults = try scratchDefaults()
        defaults.set(true, forKey: DebugOffline.defaultsKey)
        DebugOffline.applyLaunchArguments(["omnibus", "--uitest-online"], defaults: defaults)
        #expect(!defaults.bool(forKey: DebugOffline.defaultsKey))
    }

    /// The relaunch case. An agent kills the app to prove a queued write is
    /// durable, and a launch that quietly cleared the switch would put the app
    /// back online and drain the queue before the agent could look at it.
    @Test("no argument leaves a persisted switch alone")
    func silenceLeavesTheSwitchAlone() throws {
        let defaults = try scratchDefaults()
        defaults.set(true, forKey: DebugOffline.defaultsKey)
        DebugOffline.applyLaunchArguments(["omnibus", "--uitest-reset"], defaults: defaults)
        #expect(defaults.bool(forKey: DebugOffline.defaultsKey))
    }

    @Test("offline wins when both arguments are passed")
    func offlineWinsOverOnline() throws {
        let defaults = try scratchDefaults()
        DebugOffline.applyLaunchArguments(
            ["omnibus", "--uitest-online", "--uitest-offline"], defaults: defaults
        )
        #expect(defaults.bool(forKey: DebugOffline.defaultsKey))
    }
}

#endif
