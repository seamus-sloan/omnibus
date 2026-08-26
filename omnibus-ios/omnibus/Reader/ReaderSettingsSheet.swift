//  ReaderSettingsSheet.swift
//  Reading typography: page theme, face, size, leading, margins.
//
//  Was a stock `Form` of system segmented controls — grey chrome from a
//  different app, on the one surface where the reader is most likely to be
//  looking closely at how things are set. Themes are now shown in the page
//  colours they actually produce, so picking one is looking rather than
//  guessing.

import SwiftUI

struct ReaderSettingsSheet: View {
    @Bindable var controller: ReaderController

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    private let themes: [(token: String, label: String)] = [
        ("light", "Light"), ("sepia", "Sepia"), ("dark", "Dark"), ("black", "Black"),
    ]
    private let fonts: [(token: String, label: String)] = [
        ("serif", "Serif"), ("sans-serif", "Sans"), ("monospace", "Mono"),
    ]

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 26) {
                    group("Page") {
                        HStack(spacing: Spacing.md) {
                            ForEach(themes, id: \.token) { theme in
                                PageSwatch(
                                    token: theme.token,
                                    label: theme.label,
                                    isOn: controller.settings.theme == theme.token
                                ) {
                                    guard controller.settings.theme != theme.token else { return }
                                    Haptics.select()
                                    withAnimation(Motion.settle) {
                                        controller.settings.theme = theme.token
                                    }
                                }
                            }
                        }
                    }

                    group("Type") {
                        PillSelector(
                            options: fonts.map(\.token),
                            label: { token in
                                fonts.first { $0.token == token }?.label ?? token
                            },
                            selection: $controller.settings.fontFamily
                        )

                        Plate {
                            sizeRow
                            lineHeightRow
                        }
                    }

                    group("Layout") {
                        PillSelector(
                            options: ReaderMargins.allCases,
                            label: \.label,
                            selection: $controller.settings.margins
                        )

                        PillSelector(
                            options: ReaderSpread.allCases,
                            label: \.label,
                            selection: $controller.settings.spread
                        )

                        Plate {
                            PlateRow(label: "Justify text", isFirst: true) {
                                Toggle("", isOn: $controller.settings.justify)
                                    .labelsHidden()
                                    .tint(palette.accentColor)
                            }
                        }
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 32)
            }
            .scrollIndicators(.hidden)
            .background(ScreenBackground())
            .navigationTitle("Reading")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        // Tall enough that every control is on screen at rest — a `.medium`
        // detent cut the layout group off, so half the settings needed a drag
        // to discover — while still leaving page enough of the page visible to
        // watch a change land.
        .presentationDetents([.height(600), .large])
        .tint(palette.accentColor)
    }

    /// The size is set in the face it controls, so the control previews itself.
    /// The number carries what the preview can't: two adjacent sizes look alike,
    /// so a change that didn't land is otherwise indistinguishable from one that
    /// did — and it matches the line-height row directly beneath.
    private var sizeRow: some View {
        PlateRow(label: "Size", isFirst: true) {
            HStack(spacing: Spacing.md) {
                Text("Aa")
                    .font(.display(CGFloat(controller.settings.fontSize)))
                    .foregroundStyle(palette.ink1Color)
                    .frame(width: 46, alignment: .trailing)
                    .animation(Motion.snap, value: controller.settings.fontSize)

                Text("\(controller.settings.fontSize)")
                    .font(.monoUI(12))
                    .foregroundStyle(palette.ink2Color)
                    .contentTransition(.numericText())
                    // The row is tight enough that the number is what SwiftUI
                    // compresses first — without this it wraps to a digit per
                    // line rather than pushing on anything else.
                    .lineLimit(1)
                    .fixedSize()
                    .animation(Motion.snap, value: controller.settings.fontSize)

                stepButton("minus", enabled: controller.settings.fontSize > 12) {
                    controller.settings.fontSize = max(12, controller.settings.fontSize - 1)
                }
                stepButton("plus", enabled: controller.settings.fontSize < 34) {
                    controller.settings.fontSize = min(34, controller.settings.fontSize + 1)
                }
            }
        }
    }

    private var lineHeightRow: some View {
        VStack(spacing: 0) {
            Hairline()

            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    RowLabel("Line height")
                    Spacer(minLength: 0)
                    Text(String(format: "%.1f", controller.settings.lineHeight))
                        .font(.monoUI(12))
                        .foregroundStyle(palette.ink2Color)
                        .contentTransition(.numericText())
                }
                Slider(value: $controller.settings.lineHeight, in: 1.2...2.2, step: 0.1)
                    .tint(palette.accentColor)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
        }
    }

    private func stepButton(
        _ icon: String, enabled: Bool, action: @escaping () -> Void
    ) -> some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(enabled ? palette.accentColor : palette.ink3Color)
                .frame(width: 32, height: 32)
                .background(Circle().fill(palette.bg2Color))
                .overlay(Circle().strokeBorder(palette.line2.color, lineWidth: 0.5))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private func group<Content: View>(
        _ title: String, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel(title)
            content()
        }
    }
}

/// One reading theme, shown as the page it produces.
private struct PageSwatch: View {
    let token: String
    let label: String
    let isOn: Bool
    let action: () -> Void

    @Environment(\.palette) private var palette

    private var page: Color {
        switch token {
        case "light": Palette.light.readerPage
        case "sepia": Palette.sepia.readerPage
        case "black": Palette.black.readerPage
        default: Palette.atrium.readerPage
        }
    }

    private var ink: Color {
        token == "light" || token == "sepia"
            ? Palette.light.ink0Color
            : Palette.atrium.ink0Color
    }

    var body: some View {
        Button(action: action) {
            VStack(spacing: 7) {
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(page)
                    .aspectRatio(0.82, contentMode: .fit)
                    .overlay {
                        VStack(alignment: .leading, spacing: 3.5) {
                            ForEach([1.0, 0.86, 0.94, 0.6], id: \.self) { fraction in
                                Capsule()
                                    .fill(ink.opacity(0.6))
                                    .frame(width: 30 * fraction, height: 2.5)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .center)
                    }
                    .overlay(
                        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                            .strokeBorder(
                                isOn ? palette.accentColor : palette.lineColor,
                                lineWidth: isOn ? 2 : 0.75
                            )
                    )

                Text(label)
                    .font(.ui(11, weight: isOn ? .semibold : .regular))
                    .foregroundStyle(isOn ? palette.ink0Color : palette.ink3Color)
            }
        }
        .buttonStyle(PressableStyle())
        .accessibilityLabel(label)
        .accessibilityAddTraits(isOn ? [.isSelected, .isButton] : .isButton)
    }
}

