//  RootView.swift
//  Routes between the connect / login / main phases.

import SwiftUI

struct RootView: View {
    @Environment(AppState.self) private var app
    @Environment(\.palette) private var palette

    private var router: DeepLinkRouter { DeepLinkRouter.shared }

    var body: some View {
        ZStack {
            ScreenBackground()

            switch app.phase {
            case .launching:
                LaunchView()
            case .needsServer:
                ServerConnectView()
                    .transition(.opacity.combined(with: .scale(scale: 0.98)))
            case .needsLogin:
                LoginView()
                    .transition(.opacity.combined(with: .scale(scale: 0.98)))
            case .ready:
                MainTabView()
                    .transition(.opacity)
            }
        }
        .animation(Motion.glide, value: app.phase)
        .onOpenURL { router.receive($0) }
        // Keyed on both halves, so the order they arrive in doesn't matter: a
        // cold launch from a widget delivers the URL first and reaches `.ready`
        // second, while a tap with the app already open does the reverse.
        .task(id: DeepLinkDelivery(phase: app.phase, link: router.pending)) {
            guard app.phase == .ready else { return }
            await router.deliverPending()
        }
    }
}

/// The pair `.task` watches — a value type so SwiftUI can tell one delivery
/// attempt from the next.
private struct DeepLinkDelivery: Equatable {
    let phase: AppState.Phase
    let link: DeepLink?
}

/// Shown for the fraction of a second the store takes to open. Matches the
/// launch ground so there is no flash between the launch screen and the app.
private struct LaunchView: View {
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: Spacing.lg) {
            OmnibusMark(height: 44)
            ProgressView()
                .tint(palette.ink3Color)
        }
        // The mark is decorative and the spinner has no label of its own, so
        // without this the launching phase announces as a bare busy indicator.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Loading Omnibus")
    }
}
