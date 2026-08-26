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
        // The router owns the delivery itself; this only tells it when the app
        // is somewhere a book can be opened. Deliberately not a `.task(id:)`
        // over the router's own state — SwiftUI cancels a view task whenever
        // anything it observes changes, which cancelled the delivery the moment
        // it started. `initial: true` covers the app launching straight into
        // `.ready` from a cached identity.
        .onChange(of: app.phase, initial: true) { _, phase in
            router.setReady(phase == .ready)
        }
    }
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
