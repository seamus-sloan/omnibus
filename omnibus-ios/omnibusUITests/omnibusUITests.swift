//  omnibusUITests.swift
//  Launch-path smoke tests. Hermetic and offline: every launch passes
//  --uitest-reset (see AppState.init) so the app boots to the server-connect
//  phase no matter what state the simulator carries, and nothing here talks
//  to a server — the one "connect" attempt uses input that fails client-side
//  validation before any request is made.

import XCTest

final class ConnectSmokeTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchToConnect() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["--uitest-reset"]
        app.launch()
        return app
    }

    func testFreshLaunchLandsOnServerConnect() throws {
        let app = launchToConnect()

        XCTAssertTrue(
            app.staticTexts["Connect to Omnibus"].waitForExistence(timeout: 15),
            "server-connect phase should render after a reset launch"
        )
        let connect = app.buttons["Connect"]
        XCTAssertTrue(connect.exists)
        // Empty address keeps the CTA disabled.
        XCTAssertFalse(connect.isEnabled)
    }

    func testTypingAnAddressEnablesConnect() throws {
        let app = launchToConnect()
        XCTAssertTrue(app.staticTexts["Connect to Omnibus"].waitForExistence(timeout: 15))

        // The connect screen has exactly one text field.
        let field = app.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.typeText("192.0.2.1:3000")

        XCTAssertTrue(app.buttons["Connect"].isEnabled)
    }

    func testInvalidAddressShowsValidationError() throws {
        let app = launchToConnect()
        XCTAssertTrue(app.staticTexts["Connect to Omnibus"].waitForExistence(timeout: 15))

        let field = app.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        // "///" normalizes to a URL with no host, which ServerURLStore.normalize
        // rejects before any network probe.
        field.typeText("///")
        app.buttons["Connect"].tap()

        XCTAssertTrue(
            app.staticTexts["That doesn't look like a valid address."]
                .waitForExistence(timeout: 5)
        )
    }
}
