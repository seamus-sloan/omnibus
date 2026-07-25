//
//  omnibusApp.swift
//  omnibus
//

import AVFoundation
import SwiftUI
import UIKit

/// Exists for one callback: iOS relaunches the app in the background to report
/// finished downloads, and it has to be told when the app is done with them or
/// it counts the wake-up as time wasted and grants fewer of them.
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        handleEventsForBackgroundURLSession identifier: String,
        completionHandler: @escaping () -> Void
    ) {
        guard identifier == DownloadManager.sessionID else {
            completionHandler()
            return
        }
        // Touching the singleton is what reattaches the session, which is how
        // the queued completions get delivered at all.
        DownloadManager.shared.backgroundCompletion = completionHandler
    }
}

@main
struct omnibusApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var appState = AppState()
    @Environment(\.scenePhase) private var scenePhase

    init() {
        // Configure the audio session once at launch so playback keeps running
        // behind the lock screen and routes to AirPlay / CarPlay correctly.
        try? AVAudioSession.sharedInstance().setCategory(
            .playback, mode: .spokenAudio, policy: .longFormAudio
        )
        Appearance.apply()
        // Has to happen before launch finishes, so it can't wait for the
        // scene-phase handler below.
        LifecycleSync.shared.registerBackgroundTask()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(appState)
                .environment(AudioPlayer.shared)
                .themed(appState.palette, scheme: appState.theme.colorScheme)
                .task { await appState.bootstrap() }
                // Coming back and going away are the two moments a reader
                // thinks of as syncing. `.task` above fires once at launch and
                // never again, so without this the app only ever synced on a
                // cold start, a sign-in, or a reconnect it happened to notice.
                .onChange(of: scenePhase) { _, phase in
                    guard appState.phase == .ready else { return }
                    switch phase {
                    case .active:
                        Task { await LifecycleSync.shared.didEnterForeground() }
                    case .background:
                        Task { await LifecycleSync.shared.didEnterBackground() }
                    default:
                        break
                    }
                }
        }
    }
}
