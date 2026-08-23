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
    /// Note this is *not* the clock that was failing these tests on CI — that
    /// was XCUITest's own snapshot budget, which no test-side timeout governs
    /// (see the `firstMatch` note below). The headroom over the original 60s
    /// is for genuinely slow renders: a passing run costs 67–131s on CI
    /// against 10–15s locally.
    private let renderTimeout: TimeInterval = 120

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchToShell() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["--uitest-reset", "--uitest-shell"]
        app.launch()
        return app
    }

    /// Every query here resolves through `firstMatch`, and that is load-bearing
    /// rather than stylistic.
    ///
    /// A plain `app.buttons["x"]` or `.element` has to evaluate *every* match to
    /// prove the one it returns is unique, so its cost scales with the whole
    /// accessibility hierarchy. With the keyboard up that hierarchy gains a
    /// button per key, and on CI the resulting snapshot blew XCUITest's own
    /// internal evaluation budget — `Failed to get matching snapshots: Timed
    /// out while evaluating UI query`, which is thrown from inside the
    /// framework and which no test-side timeout can extend. `firstMatch` stops
    /// at the first hit and never pays for the rest.
    ///
    /// Measured on the Library item, never Search: with the keyboard up its
    /// return key is itself a button identified "Search", so that query is
    /// ambiguous. The four items share a row, so any one fixes the bar's y.
    private func tabBarTop(_ app: XCUIApplication) -> CGFloat {
        app.buttons["Library"].firstMatch.frame.minY
    }

    /// Switch tabs, tolerating a tap that lands during the shell's entry
    /// transition and is dropped. Two attempts with a full render's patience
    /// each, rather than many short ones — on a slow runner a short wait
    /// retries a tab that was already switching, which is both wrong and
    /// expensive.
    private func switchTab(_ app: XCUIApplication, to tab: String, expecting: String) -> Bool {
        let item = app.buttons[tab].firstMatch
        guard item.waitForExistence(timeout: renderTimeout) else { return false }
        for _ in 0..<2 {
            item.tap()
            if app.staticTexts[expecting].firstMatch.waitForExistence(timeout: renderTimeout) {
                return true
            }
        }
        return false
    }

    /// The keyboard, resolved the cheap way. `app.keyboards.element` is the
    /// single most expensive query in this file — it resolves the full key
    /// hierarchy — and it is the one the CI failure pointed at.
    private func keyboard(_ app: XCUIApplication) -> XCUIElement {
        app.keyboards.firstMatch
    }

    /// #2102: the keyboard belongs over the tab bar. Before the fix the
    /// keyboard's bottom safe-area inset lifted the whole `safeAreaInset`
    /// block, parking the tabs on top of the keyboard ~90pt clear of where
    /// they belong.
    func testKeyboardCoversTheTabBarRatherThanLiftingIt() throws {
        let app = launchToShell()
        XCTAssertTrue(switchTab(app, to: "Search", expecting: "Browse"), "search tab should render")

        // Addressed by identifier rather than `textFields.firstMatch`: the
        // Search screen carries exactly one field, so naming it keeps the
        // query keyed instead of positional.
        let field = app.textFields["search-query"].firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: renderTimeout), "search field should render")
        let restingTop = tabBarTop(app)

        field.tap()
        XCTAssertTrue(
            keyboard(app).waitForExistence(timeout: renderTimeout),
            "keyboard should come up"
        )

        let keyboardTop = keyboard(app).frame.minY
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
    /// identifier — an unnamed `textFields` query on a screen this dense is
    /// ambiguous about which field it lands on.
    func testFieldInsideATabStaysAboveTheKeyboard() throws {
        let app = launchToShell()
        XCTAssertTrue(
            switchTab(app, to: "You", expecting: "Send to Kindle"), "you tab should render"
        )

        let field = app.textFields["kindle-email"].firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: renderTimeout), "kindle field should render")
        for _ in 0..<3 where !field.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(field.isHittable, "kindle field should be reachable by scrolling")

        field.tap()
        XCTAssertTrue(
            keyboard(app).waitForExistence(timeout: renderTimeout), "keyboard should come up"
        )

        XCTAssertLessThanOrEqual(
            field.frame.maxY, keyboard(app).frame.minY,
            "a focused field must stay above the keyboard"
        )
    }
}
