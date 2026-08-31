//  EdgeSwipeBack.swift
//  Keeps the edge-swipe back gesture alive on screens that hide the system
//  navigation bar.
//
//  `UINavigationController` gates its interactive pop on an internal delegate
//  that refuses the gesture while the bar is hidden — so a screen drawing its
//  own chrome (the book detail marquee) silently loses the most ingrained
//  back gesture on the platform. Re-pointing the recognizer at a delegate of
//  our own restores it.

import SwiftUI
import UIKit

extension View {
    /// Re-enables the enclosing navigation controller's edge-swipe back
    /// gesture. Attach to any screen that hides the navigation bar.
    func keepsEdgeSwipeBack() -> some View {
        background(EdgeSwipeBackEnabler())
    }
}

/// The recognizer's replacement delegate: allow the pop whenever the stack
/// has somewhere to pop to and no transition is mid-flight — the same answer
/// the system gives with the bar visible.
///
/// Stateless and shared, deliberately: `UIGestureRecognizer.delegate` is
/// weak, so a per-view delegate that deallocates on pop would leave the
/// recognizer with no gate at all — and an ungated pop on a root screen
/// wedges the stack. The navigation controller is resolved per-call from the
/// recognizer's own view, so one instance serves every tab's stack.
final class EdgeSwipeBackDelegate: NSObject, UIGestureRecognizerDelegate {
    static let shared = EdgeSwipeBackDelegate()

    func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        guard let navigationController = Self.navigationController(of: gestureRecognizer)
        else { return false }
        return navigationController.viewControllers.count > 1
            && navigationController.transitionCoordinator == nil
    }

    /// The navigation controller owning this recognizer, via the responder
    /// chain from the recognizer's view.
    static func navigationController(of recognizer: UIGestureRecognizer)
        -> UINavigationController?
    {
        var responder: UIResponder? = recognizer.view
        while let current = responder {
            if let navigationController = current as? UINavigationController {
                return navigationController
            }
            responder = current.next
        }
        return nil
    }
}

/// Invisible representable that walks up to the enclosing navigation
/// controller once it lands in the hierarchy and installs the delegate.
private struct EdgeSwipeBackEnabler: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Proxy { Proxy() }
    func updateUIViewController(_ proxy: Proxy, context: Context) {}

    final class Proxy: UIViewController {
        override func didMove(toParent parent: UIViewController?) {
            super.didMove(toParent: parent)
            install()
        }

        // `didMove` can fire before the navigation controller is reachable;
        // by first appearance it always is.
        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            install()
        }

        private func install() {
            guard let recognizer = enclosingNavigationController?
                .interactivePopGestureRecognizer
            else { return }
            recognizer.delegate = EdgeSwipeBackDelegate.shared
            recognizer.isEnabled = true
        }

        private var enclosingNavigationController: UINavigationController? {
            var responder: UIResponder? = self
            while let current = responder {
                if let navigationController = current as? UINavigationController {
                    return navigationController
                }
                responder = current.next
            }
            return nil
        }
    }
}
