//  Plate.swift
//  Section headings, plates, records, fields, and the pill selector.
//
//  These started life on the book detail screen and are now the app's shared
//  vocabulary for "a set of labelled values": the metadata editor, the shelf
//  picker, and the shelf composer all build from them, so they live in the
//  design layer rather than inside one feature.
//
//  Two grounds, deliberately: a `Plate` is filled and reads as a form — right
//  when you're editing — while a `Record` is bare hairlines on the page ground,
//  right when you're reading.

import SwiftUI

/// Section heading, in the display face.
struct SectionLabel: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    @Environment(\.palette) private var palette

    var body: some View {
        Text(title)
            .font(.display(23))
            .foregroundStyle(palette.ink0Color)
            .accessibilityAddTraits(.isHeader)
    }
}

// MARK: - Grounds

/// Wraps rows in the plate's filled ground.
struct Plate<Content: View>: View {
    @ViewBuilder var content: () -> Content

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            content()
        }
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .fill(palette.bg1Color)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(palette.line2.color, lineWidth: 0.5)
        )
    }
}

/// One row of the filled plate.
struct PlateRow<Content: View>: View {
    let label: String
    var isFirst = false
    @ViewBuilder var content: () -> Content

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                RowLabel(label)
                Spacer(minLength: Spacing.sm)
                content()
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 13)
        }
    }
}

/// A row of the borderless record: label, trailing content, hairline rule.
struct RecordRow<Content: View>: View {
    let label: String
    var isFirst = false
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                RowLabel(label)
                Spacer(minLength: Spacing.sm)
                content()
            }
            .padding(.vertical, 14)
        }
    }
}

/// Closes a record with a rule, so the last row doesn't dangle.
struct RecordRule: View {
    var body: some View { Hairline() }
}

struct Hairline: View {
    @Environment(\.palette) private var palette

    var body: some View {
        Rectangle()
            .fill(palette.line2.color)
            .frame(height: 0.5)
    }
}

/// The mono small-caps key every row leads with.
struct RowLabel: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    @Environment(\.palette) private var palette

    var body: some View {
        Text(text.uppercased())
            .font(.monoUI(10, weight: .medium))
            .tracking(0.7)
            .foregroundStyle(palette.ink3Color)
    }
}

// MARK: - Editable field

/// A labelled text field inside a plate.
///
/// The label is always present — not a placeholder that vanishes the moment the
/// field has a value — and can carry an accent marker when the value differs
/// from what was loaded.
struct PlateField: View {
    let label: String
    @Binding var text: String
    var isEdited = false
    var isFirst = false
    var hint: String?
    var keyboard: UIKeyboardType = .default
    var multiline = false
    /// Focus this field as soon as the form appears — for the one field a sheet
    /// exists to collect.
    var autofocus = false
    /// Autocomplete pool. Non-empty turns the field into the single-value
    /// counterpart of the chip editor's dropdown (the web's `SuggestField`):
    /// picking a row overwrites the value, and there is no "+ Create" row
    /// since typing already edits the value directly.
    var suggestions: [SuggestionItem] = []
    /// Stable handle for omnibusUITests. The field has no label of its own
    /// (the caption above it is a sibling), so without one a test can only
    /// reach it positionally.
    var identifier: String?

    @Environment(\.palette) private var palette
    @FocusState private var focused: Bool
    /// Whether the dropdown may show — reopened by focus or a keystroke,
    /// closed by a pick. Needed because the just-picked value is often a
    /// substring of other pool entries, so "filtered is non-empty" alone
    /// would keep the dropdown open forever.
    @State private var open = false
    /// The name a pick just wrote into `text`, so its `onChange` echo can be
    /// told apart from a keystroke and not reopen the dropdown.
    @State private var lastPicked: String?

    var body: some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }

            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(label.uppercased())
                        .font(.monoUI(10, weight: .medium))
                        .tracking(0.7)
                        .foregroundStyle(isEdited ? palette.accentColor : palette.ink3Color)

                    if isEdited {
                        Circle()
                            .fill(palette.accentColor)
                            .frame(width: 4, height: 4)
                            .transition(.scale.combined(with: .opacity))
                    }

                    Spacer(minLength: 0)

                    if let hint, !isEdited {
                        Text(hint)
                            .font(.ui(10.5))
                            .foregroundStyle(palette.ink3Color.opacity(0.8))
                    }
                }

                TextField("", text: $text, axis: multiline ? .vertical : .horizontal)
                    .font(multiline ? .display(18) : .ui(15))
                    .foregroundStyle(palette.ink0Color)
                    .textInputAutocapitalization(multiline ? .sentences : .words)
                    .autocorrectionDisabled(!multiline)
                    .keyboardType(keyboard)
                    // Grows with what's in it. A four-line floor left an empty
                    // optional field looking like the form's main event.
                    .lineLimit(multiline ? 2...12 : 1...1)
                    .tint(palette.accentColor)
                    .focused($focused)
                    .accessibilityIdentifier(identifier ?? "")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .animation(Motion.snap, value: isEdited)

            if open, !suggestions.isEmpty {
                let rows = SuggestionPool.filtered(
                    pool: suggestions, current: [text], query: text
                )
                if !rows.isEmpty {
                    SuggestionList(items: rows) { name in
                        lastPicked = name
                        text = name
                        open = false
                    }
                    .padding(.bottom, 6)
                }
            }
        }
        .onAppear {
            guard autofocus else { return }
            focused = true
        }
        .onChange(of: focused) { _, isFocused in
            open = isFocused
        }
        .onChange(of: text) { _, newValue in
            if newValue == lastPicked {
                lastPicked = nil
                return
            }
            if focused { open = true }
        }
    }
}

// MARK: - Pill selector

/// A segmented control with a sliding accent indicator.
///
/// The system's `.segmented` picker can't be tinted per-theme and animates with
/// its own curve, so it always looked like a control borrowed from a different
/// app. This one moves on the app's own `snap`.
struct PillSelector<Value: Hashable>: View {
    let options: [Value]
    let label: (Value) -> String
    @Binding var selection: Value
    var onChange: (Value) -> Void = { _ in }

    @Environment(\.palette) private var palette
    @Namespace private var indicator

    var body: some View {
        HStack(spacing: 4) {
            ForEach(options, id: \.self) { option in
                segment(option)
            }
        }
        .padding(3)
        .background(
            Capsule().fill(palette.bg2Color.opacity(0.7))
        )
    }

    private func segment(_ option: Value) -> some View {
        let isOn = selection == option

        return Button {
            guard selection != option else { return }
            Haptics.select()
            withAnimation(Motion.snap) { selection = option }
            onChange(option)
        } label: {
            Text(label(option))
                .font(.ui(13, weight: isOn ? .semibold : .medium))
                .foregroundStyle(isOn ? palette.accentInk.color : palette.ink2Color)
                .lineLimit(1)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 7)
                .background {
                    if isOn {
                        Capsule()
                            .fill(palette.accentColor)
                            .matchedGeometryEffect(id: "pill", in: indicator)
                    }
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? [.isSelected, .isButton] : .isButton)
    }
}
