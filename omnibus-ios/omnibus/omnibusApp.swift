//
//  omnibusApp.swift
//  omnibus
//

import AVFoundation
import SwiftUI

@main
struct omnibusApp: App {
    @State private var appState = AppState()

    init() {
        // Configure the audio session once at launch so playback keeps running
        // behind the lock screen and routes to AirPlay / CarPlay correctly.
        try? AVAudioSession.sharedInstance().setCategory(
            .playback, mode: .spokenAudio, policy: .longFormAudio
        )
        Appearance.apply()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(appState)
                .environment(AudioPlayer.shared)
                .themed(appState.palette, scheme: appState.theme.colorScheme)
                .task { await appState.bootstrap() }
        }
    }
}
