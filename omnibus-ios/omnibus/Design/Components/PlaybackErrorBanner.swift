//  PlaybackErrorBanner.swift
//  Playback stopped, and the transport stays usable.
//
//  A sibling of `SyncOfferBanner` rather than `ErrorStateView`: a failed item
//  mid-book is not a screen that failed to load, and covering the transport
//  with a full-page error would take away the play button that is the way out
//  of it (#2408). The listener keeps their position, their chapter list, and
//  a Retry that rebuilds the item where they left off.

import SwiftUI

struct PlaybackErrorBanner: View {
    let message: String
    let onRetry: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack {
            Spacer(minLength: 0)

            HStack(spacing: Spacing.md) {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(palette.badColor)

                Text(message)
                    .font(.ui(11.5))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize(horizontal: false, vertical: true)

                Spacer(minLength: 0)

                Button("Retry", action: onRetry)
                    .font(.ui(13, weight: .semibold))
                    .foregroundStyle(palette.accentColor)
            }
            .padding(.horizontal, Spacing.md)
            .padding(.vertical, Spacing.sm)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(.ultraThinMaterial)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .strokeBorder(palette.badColor.opacity(0.4), lineWidth: 0.5)
            )
            .padding(.horizontal, Spacing.md)
            .padding(.bottom, 64)
        }
    }
}
