//  EdgeSwipeBackTests.swift
//  The restored edge-swipe back gesture's gate: pop only when the stack has
//  somewhere to pop to — an ungated pop on a root screen wedges the stack.

import Testing
import UIKit

@testable import omnibus

@Test @MainActor func edgeSwipeBeginsOnlyWithSomewhereToPop() throws {
    let navigationController = UINavigationController(rootViewController: UIViewController())
    navigationController.loadViewIfNeeded()
    let recognizer = try #require(navigationController.interactivePopGestureRecognizer)

    // At the root there is nowhere to pop to.
    #expect(!EdgeSwipeBackDelegate.shared.gestureRecognizerShouldBegin(recognizer))

    navigationController.pushViewController(UIViewController(), animated: false)
    #expect(EdgeSwipeBackDelegate.shared.gestureRecognizerShouldBegin(recognizer))
}

@Test @MainActor func edgeSwipeResolvesItsOwnNavigationController() throws {
    // Two stacks, one shared delegate: each recognizer answers for its own
    // stack, so one tab's depth can't leak into another's.
    let deep = UINavigationController(rootViewController: UIViewController())
    deep.loadViewIfNeeded()
    deep.pushViewController(UIViewController(), animated: false)
    let shallow = UINavigationController(rootViewController: UIViewController())
    shallow.loadViewIfNeeded()

    let deepRecognizer = try #require(deep.interactivePopGestureRecognizer)
    let shallowRecognizer = try #require(shallow.interactivePopGestureRecognizer)

    #expect(EdgeSwipeBackDelegate.navigationController(of: deepRecognizer) === deep)
    #expect(EdgeSwipeBackDelegate.shared.gestureRecognizerShouldBegin(deepRecognizer))
    #expect(!EdgeSwipeBackDelegate.shared.gestureRecognizerShouldBegin(shallowRecognizer))
}
