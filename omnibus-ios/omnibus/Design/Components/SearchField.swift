//  SearchField.swift
//  The app's own search field.
//
//  `.searchable` renders a UIKit bar drawer and drags the stock large title
//  along with it, which is why the Search tab was the one screen that stopped
//  looking like the rest of the app. This is the same behaviour — focus ring,
//  clear, cancel, keyboard dismissal — drawn in the Atrium palette, so Search
//  can lead with the masthead every other tab leads with.

import SwiftUI

struct SearchField: View {
    @Binding var text: String
    var prompt: String
    var onSubmit: () -> Void = {}

    @Environment(\.palette) private var palette
    @FocusState private var focused: Bool

    private var isActive: Bool { focused || !text.isEmpty }

    var body: some View {
        HStack(spacing: Spacing.md) {
            field

            if isActive {
                Button("Cancel") {
                    text = ""
                    focused = false
                }
                .font(.ui(14.5))
                .foregroundStyle(palette.accentColor)
                .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .animation(Motion.snap, value: isActive)
    }

    private var field: some View {
        HStack(spacing: 9) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(isActive ? palette.accentColor : palette.ink3Color)

            TextField("", text: $text, prompt: promptText)
                .font(.ui(16))
                .foregroundStyle(palette.ink0Color)
                .tint(palette.accentColor)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .focused($focused)
                .onSubmit(onSubmit)

            if !text.isEmpty {
                Button {
                    text = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 15))
                        .foregroundStyle(palette.ink3Color)
                }
                .buttonStyle(.plain)
                .transition(.scale.combined(with: .opacity))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(Capsule().fill(palette.bg1Color))
        .overlay(
            Capsule().strokeBorder(
                focused ? palette.accentColor.opacity(0.55) : palette.line2.color,
                lineWidth: focused ? 1 : 0.5
            )
        )
        // Tapping anywhere in the capsule should take the caret, not just the
        // ~200pt of it the text field itself occupies.
        .contentShape(Capsule())
        .onTapGesture { focused = true }
    }

    private var promptText: Text {
        Text(prompt).foregroundStyle(palette.ink3Color)
    }
}
