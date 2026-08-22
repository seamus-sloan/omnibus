//  ShellChromeTests.swift
//  Chrome contracts for the signed-in shell. Hermetic and offline like the
//  connect smoke tests: `--uitest-shell` seeds a server, token, and cached
//  identity so `MainTabView` renders with no server behind it (AppState.init /
//  bootstrap). Nothing here reads library content — only the shell's own
//  chrome, which is local.

import XCTest

final class ShellChromeTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchToShell() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["--uitest-reset", "--uitest-shell"]
        app.launch()
        return app
    }

    /// Measured on the Library item, never Search: with the keyboard up its
    /// return key is itself a button identified "Search", so that query is
    /// ambiguous. The four items share a row, so any one fixes the bar's y.
    private func tabBarTop(_ app: XCUIApplication) -> CGFloat {
        app.buttons["Library"].frame.minY
    }

    /// Tapping a tab during the shell's entry transition is dropped, so retry
    /// until the destination actually renders.
    private func switchTab(_ app: XCUIApplication, to tab: String, expecting: String) -> Bool {
        let item = app.buttons[tab]
        guard item.waitForExistence(timeout: 30) else { return false }
        for _ in 0..<5 {
            item.tap()
            if app.staticTexts[expecting].waitForExistence(timeout: 6) { return true }
        }
        return false
    }

    /// #2102: the keyboard belongs over the tab bar. Before the fix the
    /// keyboard's bottom safe-area inset lifted the whole `safeAreaInset`
    /// block, parking the tabs on top of the keyboard ~90pt clear of where
    /// they belong.
    func testKeyboardCoversTheTabBarRatherThanLiftingIt() throws {
        let app = launchToShell()
        XCTAssertTrue(switchTab(app, to: "Search", expecting: "Browse"), "search tab should render")

        let field = app.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 15), "search field should render")
        let restingTop = tabBarTop(app)

        field.tap()
        XCTAssertTrue(app.keyboards.element.waitForExistence(timeout: 15), "keyboard should come up")

        let keyboardTop = app.keyboards.element.frame.minY
        XCTAssertGreaterThanOrEqual(
            tabBarTop(app), keyboardTop,
            "tab bar must sit at or below the keyboard top, not above it"
        )
        XCTAssertEqual(
            tabBarTop(app), restingTop, accuracy: 1,
            "tab bar must not move when the keyboard opens"
        )
    }

    /// The other half of the contract: pinning the shell must not cost a
    /// focused field inside a tab its keyboard avoidance.
    func testFieldInsideATabStaysAboveTheKeyboard() throws {
        let app = launchToShell()
        XCTAssertTrue(
            switchTab(app, to: "You", expecting: "Send to Kindle"), "you tab should render"
        )

        let field = app.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 15), "kindle email field should render")
        for _ in 0..<6 where !field.isHittable {
            app.swipeUp()
        }
        field.tap()
        XCTAssertTrue(app.keyboards.element.waitForExistence(timeout: 15))

        XCTAssertLessThanOrEqual(
            field.frame.maxY, app.keyboards.element.frame.minY,
            "a focused field must stay above the keyboard"
        )
    }
}
