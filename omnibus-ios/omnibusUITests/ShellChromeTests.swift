//  ShellChromeTests.swift
//  Chrome contracts for the signed-in shell. Hermetic and offline like the
//  connect smoke tests: `--uitest-shell` seeds a server, token, and cached
//  identity so `MainTabView` renders with no server behind it (AppState.init /
//  bootstrap). Nothing here reads library content — only the shell's own
//  chrome, which is local.
//
//  Timeouts are deliberately generous. A loaded CI runner is an order of
//  magnitude slower than a dev machine — a single app launch there can take
//  the better part of a minute — and CI runs under
//  `-retry-tests-on-failure`, so a too-tight wait reads as a real failure
//  twice over rather than as the slow render it is.

import XCTest

final class ShellChromeTests: XCTestCase {
    /// One render's worth of patience on the slowest machine this runs on.
    ///
    /// This was 60s, which is below what a *passing* run of these tests costs
    /// on CI: the green attempts measured 67.7s and 131.2s end to end, against
    /// 10–15s locally. A per-step cap under the whole-test duration trips
    /// whenever any one step lands on the slow side of that spread, which is
    /// what made both tests fail roughly two runs in three on CI while never
    /// failing here — including under a synthetic load average of 67.
    private let renderTimeout: TimeInterval = 180

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

    /// Switch tabs, tolerating a tap that lands during the shell's entry
    /// transition and is dropped. Two attempts with a full render's patience
    /// each, rather than many short ones — on a slow runner a short wait
    /// retries a tab that was already switching, which is both wrong and
    /// expensive.
    private func switchTab(_ app: XCUIApplication, to tab: String, expecting: String) -> Bool {
        let item = app.buttons[tab]
        guard item.waitForExistence(timeout: renderTimeout) else { return false }
        for _ in 0..<2 {
            item.tap()
            if app.staticTexts[expecting].waitForExistence(timeout: renderTimeout) { return true }
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

        // Addressed by identifier for the same reason the Kindle field below
        // is: `textFields.firstMatch` resolves by walking the hierarchy, and
        // the Search screen is dense enough for that to be a real cost on a
        // slow runner.
        let field = app.textFields["search-query"]
        XCTAssertTrue(field.waitForExistence(timeout: renderTimeout), "search field should render")
        let restingTop = tabBarTop(app)

        field.tap()
        XCTAssertTrue(
            app.keyboards.element.waitForExistence(timeout: renderTimeout),
            "keyboard should come up"
        )

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
    /// focused field inside a tab its keyboard avoidance. Addressed by
    /// identifier — `textFields.firstMatch` on a screen this dense is both
    /// ambiguous and expensive to resolve.
    func testFieldInsideATabStaysAboveTheKeyboard() throws {
        let app = launchToShell()
        XCTAssertTrue(
            switchTab(app, to: "You", expecting: "Send to Kindle"), "you tab should render"
        )

        let field = app.textFields["kindle-email"]
        XCTAssertTrue(field.waitForExistence(timeout: renderTimeout), "kindle field should render")
        for _ in 0..<3 where !field.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(field.isHittable, "kindle field should be reachable by scrolling")

        field.tap()
        XCTAssertTrue(app.keyboards.element.waitForExistence(timeout: renderTimeout))

        XCTAssertLessThanOrEqual(
            field.frame.maxY, app.keyboards.element.frame.minY,
            "a focused field must stay above the keyboard"
        )
    }
}
