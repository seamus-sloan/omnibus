//  ReaderSettingsTests.swift
//  A stored reader-settings blob survives the struct growing a field.
//
//  All seven typography settings share one JSON blob under
//  `omnibus.readerSettings`, and Swift's synthesized decoder ignores property
//  defaults — so before #2117 a key the stored payload didn't carry threw
//  `keyNotFound`, `load()`'s `try?` swallowed it, and the reader came back on
//  the factory theme, face, and size. Adding `spread` (#2081) shipped exactly
//  that. These tests pin the upgrade path a new field has to keep working: the
//  payloads here are deliberately every-field-off-default, so a reset is
//  unmistakable rather than accidentally matching a default.

import Foundation
import Testing

@testable import omnibus

/// A reader who has changed every setting.
private let stored = ReaderSettings(
    fontSize: 24, fontFamily: "sans-serif", lineHeight: 1.9,
    margins: .wide, justify: false, theme: "sepia", spread: .single
)

/// `stored` on the wire.
private func fullPayload() -> [String: Any] {
    [
        "fontSize": 24,
        "fontFamily": "sans-serif",
        "lineHeight": 1.9,
        "margins": "wide",
        "justify": false,
        "theme": "sepia",
        "spread": "single",
    ]
}

private func decode(_ payload: [String: Any]) throws -> ReaderSettings {
    try JSONDecoder().decode(ReaderSettings.self, from: JSONSerialization.data(withJSONObject: payload))
}

/// Runs `body` against an empty `omnibus.readerSettings`, then puts back
/// whatever the test host held.
private func withCleanStore(_ body: () throws -> Void) rethrows {
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

// Serialized: every test in here shares the one `UserDefaults` key.
@Suite("Reader settings persistence", .serialized)
struct ReaderSettingsTests {
    /// The regression itself: the payload shape a build before #2084 wrote.
    @Test("a payload written before `spread` existed keeps every setting it did carry")
    func decodesTheShapeWrittenBeforeSpreadExisted() throws {
        var payload = fullPayload()
        payload.removeValue(forKey: "spread")

        let decoded = try decode(payload)

        var expected = stored
        expected.spread = ReaderSettings().spread
        #expect(decoded == expected)
        // Spelled out, because this is the one the reader notices.
        #expect(decoded.theme == "sepia")
    }

    @Test(
        "a payload missing one key keeps every other setting",
        arguments: [
            "fontSize", "fontFamily", "lineHeight", "margins", "justify", "theme", "spread",
        ]
    )
    func missingOneKeyCostsOnlyThatSetting(dropped: String) throws {
        var payload = fullPayload()
        payload.removeValue(forKey: dropped)

        let decoded = try decode(payload)

        let defaults = ReaderSettings()
        var expected = stored
        switch dropped {
        case "fontSize": expected.fontSize = defaults.fontSize
        case "fontFamily": expected.fontFamily = defaults.fontFamily
        case "lineHeight": expected.lineHeight = defaults.lineHeight
        case "margins": expected.margins = defaults.margins
        case "justify": expected.justify = defaults.justify
        case "theme": expected.theme = defaults.theme
        case "spread": expected.spread = defaults.spread
        default: Issue.record("unhandled key \(dropped)")
        }
        #expect(decoded == expected)
    }

    /// The other direction — a TestFlight rollback reading a newer blob.
    @Test("a key this build doesn't know is ignored")
    func unknownKeyIsIgnored() throws {
        var payload = fullPayload()
        payload["pageTurnAnimation"] = "slide"

        #expect(try decode(payload) == stored)
    }

    @Test("a value of the wrong type costs only its own setting")
    func mistypedValueFallsBackAlone() throws {
        var payload = fullPayload()
        payload["fontSize"] = "extra large"

        let decoded = try decode(payload)

        #expect(decoded.fontSize == ReaderSettings().fontSize)
        #expect(decoded.theme == "sepia")
        #expect(decoded.margins == .wide)
    }

    /// Also the rollback case: an enum case a newer build introduced.
    @Test("an unknown enum case costs only its own setting")
    func unknownEnumCaseFallsBackAlone() throws {
        var payload = fullPayload()
        payload["margins"] = "colossal"

        let decoded = try decode(payload)

        #expect(decoded.margins == ReaderSettings().margins)
        #expect(decoded.theme == "sepia")
        #expect(decoded.fontSize == 24)
    }

    @Test("a saved blob round-trips through UserDefaults")
    func savedBlobRoundTrips() {
        withCleanStore {
            stored.save()

            #expect(ReaderSettings.load() == stored)
        }
    }

    /// End-to-end through the real key: what the reader actually experiences on
    /// first launch after the update.
    @Test("an upgrade onto a build with a new field loads the reader's own theme")
    func upgradeLoadsTheStoredTheme() throws {
        try withCleanStore {
            var payload = fullPayload()
            payload.removeValue(forKey: "spread")
            UserDefaults.standard.set(
                try JSONSerialization.data(withJSONObject: payload),
                forKey: ReaderSettings.storageKey
            )

            let loaded = ReaderSettings.load()

            #expect(loaded.theme == "sepia")
            #expect(loaded.fontSize == 24)
            #expect(loaded.spread == .double)
        }
    }

    @Test("nothing stored loads the defaults")
    func emptyStoreLoadsDefaults() {
        withCleanStore {
            #expect(ReaderSettings.load() == ReaderSettings())
        }
    }
}
