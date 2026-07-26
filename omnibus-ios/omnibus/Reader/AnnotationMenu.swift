//  AnnotationMenu.swift
//  The highlight menu and note composer.
//
//  One menu serves both directions: a fresh selection (no colour yet) and a
//  tap on a highlight already on the page. Anchoring it to the passage rather
//  than pinning it to the bottom of the screen is what makes it read as acting
//  *on that sentence* — the Apple Books model — instead of as a global toolbar
//  that happens to be showing.

import SwiftUI

struct AnnotationMenu: View {
    /// The highlight's current colour, or `nil` for a new selection.
    var current: HighlightColor?
    var hasNote = false
    var onColor: (HighlightColor) -> Void
    var onNote: () -> Void
    var onCopy: () -> Void
    var onShare: () -> Void
    /// Only an existing highlight can be removed.
    var onRemove: (() -> Void)?

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            colorRow
            Rectangle()
                .fill(palette.line2.color)
                .frame(height: 0.5)
            actionRow
        }
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .fill(.regularMaterial)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(.white.opacity(0.10), lineWidth: 0.5)
        )
        .shadow(color: .black.opacity(0.35), radius: 18, y: 8)
    }

    private var colorRow: some View {
        HStack(spacing: 14) {
            ForEach(HighlightColor.allCases, id: \.self) { color in
                Button {
                    Haptics.select()
                    onColor(color)
                } label: {
                    Circle()
                        .fill(color.tint)
                        .frame(width: 26, height: 26)
                        .overlay(
                            Circle().strokeBorder(.white.opacity(0.28), lineWidth: 1)
                        )
                        // The ring is how you can tell what a passage already
                        // is before you change it.
                        .overlay {
                            if current == color {
                                Circle()
                                    .strokeBorder(palette.ink0Color, lineWidth: 2)
                                    .padding(-4)
                            }
                        }
                }
                .buttonStyle(.plain)
                .accessibilityLabel(color.label)
                .accessibilityAddTraits(current == color ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 13)
    }

    private var actionRow: some View {
        HStack(spacing: 0) {
            action(hasNote ? "Edit note" : "Note", icon: "square.and.pencil", action: onNote)
            action("Copy", icon: "doc.on.doc", action: onCopy)
            action("Share", icon: "square.and.arrow.up", action: onShare)
            if let onRemove {
                action("Remove", icon: "trash", tint: palette.badColor, action: onRemove)
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 8)
    }

    private func action(
        _ label: String, icon: String, tint: Color? = nil, action: @escaping () -> Void
    ) -> some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            VStack(spacing: 4) {
                Image(systemName: icon)
                    .font(.system(size: 15, weight: .medium))
                Text(label)
                    .font(.ui(10.5, weight: .medium))
                    .lineLimit(1)
            }
            .foregroundStyle(tint ?? palette.ink0Color)
            .frame(minWidth: 62)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// Places the menu beside the passage it acts on.
struct AnnotationAnchor<Content: View>: View {
    let rect: SelectionData.SelectionRect?
    @ViewBuilder var content: () -> Content

    /// The menu is a fixed shape, so a measured size would buy nothing but a
    /// frame of the menu jumping into place.
    private let menuSize = CGSize(width: 286, height: 116)
    private let gap: CGFloat = 12
    private let edge: CGFloat = 10

    var body: some View {
        GeometryReader { geometry in
            content()
                .frame(width: menuSize.width)
                .position(center(in: geometry.size))
        }
        .ignoresSafeArea(.keyboard)
    }

    private func center(in size: CGSize) -> CGPoint {
        guard let rect else {
            // No geometry (a tap epub.js couldn't locate): fall back to the
            // bottom bar position rather than dropping the menu entirely.
            return CGPoint(x: size.width / 2, y: size.height - menuSize.height / 2 - 96)
        }

        let halfWidth = menuSize.width / 2
        let halfHeight = menuSize.height / 2
        let x = min(
            max(CGFloat(rect.x + rect.width / 2), halfWidth + edge),
            size.width - halfWidth - edge
        )

        // Above the passage when it fits, below it otherwise — the menu must
        // never cover the text it is about.
        let above = CGFloat(rect.y) - gap - halfHeight
        let below = CGFloat(rect.y + (rect.height ?? 0)) + gap + halfHeight
        let minY = halfHeight + 60
        let maxY = size.height - halfHeight - 60
        let y = above >= minY ? above : min(below, maxY)

        return CGPoint(x: x, y: min(max(y, minY), maxY))
    }
}

/// Writing or editing the note attached to a highlight.
struct NoteComposer: View {
    let quote: String?
    let existing: String?
    var onSave: (String?) -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var text = ""
    @State private var didLoad = false
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.lg) {
                    if let quote = quote?.nilIfBlank {
                        // The passage, set the way the journal sets an entry —
                        // you're writing about this sentence, so it should be
                        // in front of you.
                        Text(quote)
                            .font(.display(16))
                            .foregroundStyle(palette.ink1Color)
                            .lineSpacing(5)
                            .padding(.leading, 14)
                            .overlay(alignment: .leading) {
                                Capsule()
                                    .fill(palette.accentColor)
                                    .frame(width: 2.5)
                            }
                    }

                    Plate {
                        PlateField(
                            label: "Note",
                            text: $text,
                            isFirst: true,
                            multiline: true
                        )
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 40)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .background(ScreenBackground())
            .navigationTitle(existing?.nilIfBlank == nil ? "New note" : "Edit note")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        onSave(text.trimmingCharacters(in: .whitespacesAndNewlines).nilIfBlank)
                        dismiss()
                    }
                }
            }
        }
        .onAppear {
            if !didLoad {
                didLoad = true
                text = existing ?? ""
            }
            focused = true
        }
    }
}

extension HighlightColor {
    var label: String {
        switch self {
        case .amber: "Amber"
        case .green: "Green"
        case .blue: "Blue"
        case .rose: "Rose"
        case .violet: "Violet"
        }
    }
}
