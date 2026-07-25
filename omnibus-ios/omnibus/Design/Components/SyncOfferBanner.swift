//  SyncOfferBanner.swift
//  The offer to jump to a position another device reached.
//
//  Deliberately a banner rather than an alert: the book is already open on the
//  local position and reading or listening can continue straight past this. An
//  alert would make a sync detail block the page.

import SwiftUI

struct SyncOfferBanner: View {
    var title = "Read further elsewhere"
    var detail = "Another device left off further along."
    let onGo: () -> Void
    let onDismiss: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack {
            Spacer(minLength: 0)

            HStack(spacing: Spacing.md) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.ui(13, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                    Text(detail)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink2Color)
                }

                Spacer(minLength: 0)

                Button("Go", action: onGo)
                    .font(.ui(13, weight: .semibold))
                    .foregroundStyle(palette.accentColor)

                Button {
                    onDismiss()
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(palette.ink3Color)
                }
                .accessibilityLabel("Dismiss")
            }
            .padding(.horizontal, Spacing.md)
            .padding(.vertical, Spacing.sm)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(.ultraThinMaterial)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 0.5)
            )
            .padding(.horizontal, Spacing.md)
            .padding(.bottom, 64)
        }
    }
}
