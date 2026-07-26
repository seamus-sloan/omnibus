//  CarModeView.swift
//  A driving face for the player: five targets, none of them small.
//
//  Everything here is the same `AudioPlayer` the full player drives — this adds
//  no playback behaviour, only a face you can hit without looking. Nothing that
//  needs aiming (the scrubber, the chapter list, speed, bookmarks) is reachable
//  from here on purpose.

import SwiftUI
import UIKit

struct CarModeView: View {
    let book: Book

    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: Spacing.lg) {
            header
            Spacer(minLength: 0)
            transport
            Spacer(minLength: 0)
        }
        .padding(.horizontal, Spacing.screen)
        .padding(.vertical, Spacing.lg)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.bg0Color.ignoresSafeArea())
        // A driver isn't going to tap every 30 seconds to keep the screen up,
        // and a dark screen is the whole reason they'd look down at all.
        .onAppear { UIApplication.shared.isIdleTimerDisabled = true }
        .onDisappear { UIApplication.shared.isIdleTimerDisabled = false }
    }

    private var header: some View {
        VStack(spacing: Spacing.sm) {
            HStack {
                Spacer()
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 19, weight: .semibold))
                        .foregroundStyle(palette.ink1Color)
                        .frame(width: 52, height: 52)
                        .background(Circle().fill(palette.bg2Color))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Leave car mode")
            }

            Text(player.currentChapter?.title ?? book.displayTitle)
                .font(.ui(24, weight: .semibold))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(2)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)

            Text(Format.humanDuration(Int64(max(0, player.duration - player.position))) + " left")
                .font(.ui(16))
                .monospacedDigit()
                .foregroundStyle(palette.ink2Color)
        }
    }

    private var transport: some View {
        VStack(spacing: Spacing.md) {
            HStack(spacing: Spacing.md) {
                tile("gobackward.15", label: "Back 15 seconds") { player.skip(-15) }
                tile("goforward.30", label: "Forward 30 seconds") { player.skip(30) }
            }

            Button { player.toggle() } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 64))
                    .foregroundStyle(palette.bg0Color)
                    .frame(maxWidth: .infinity)
                    .frame(height: 176)
                    .background(
                        RoundedRectangle(cornerRadius: Radius.xl, style: .continuous)
                            .fill(palette.accentColor)
                    )
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(player.isPlaying ? "Pause" : "Play")

            HStack(spacing: Spacing.md) {
                tile(
                    "backward.end.fill",
                    label: "Previous chapter",
                    enabled: player.canGoPreviousChapter
                ) { player.previousChapter() }
                tile(
                    "forward.end.fill",
                    label: "Next chapter",
                    enabled: player.canGoNextChapter
                ) { player.nextChapter() }
            }
        }
    }

    private func tile(
        _ icon: String, label: String, enabled: Bool = true, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 38))
                .foregroundStyle(enabled ? palette.ink0Color : palette.ink3Color)
                .frame(maxWidth: .infinity)
                .frame(height: 132)
                .background(
                    RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                        .fill(palette.bg1Color)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(label)
    }
}
