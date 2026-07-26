//  RootView.swift
//  Routes between the connect / login / main phases.

import SwiftUI

struct RootView: View {
    @Environment(AppState.self) private var app
    @Environment(\.palette) private var palette

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
    }
}

/// Shown for the fraction of a second the store takes to open. Matches the
/// launch ground so there is no flash between the launch screen and the app.
private struct LaunchView: View {
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: Spacing.lg) {
            Image(systemName: "books.vertical.fill")
                .font(.system(size: 44, weight: .light))
                .foregroundStyle(palette.accentColor)
            ProgressView()
                .tint(palette.ink3Color)
        }
    }
}
