//  CreateShelfSheet.swift
//  Composing a new shelf, manual or smart.
//
//  Split out of `ShelvesViews` and rebuilt on the app's own plate vocabulary —
//  it was the last stock `Form` in the shelves flow, which made creating a
//  shelf feel like leaving the app to visit Settings.

import SwiftUI

struct CreateShelfSheet: View {
    var onCreated: () -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var description = ""
    @State private var kind: ShelfKind = .manual
    @State private var visibility: ShelfVisibility = .private
    @State private var matchMode: MatchMode = .all
    @State private var rules: [ShelfRule] = [ShelfRule(field: .tag, op: .is, value: "")]
    @State private var isSaving = false
    @State private var error: String?
    @State private var preview: RulePreview?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 28) {
                    Plate {
                        PlateField(label: "Name", text: $name, isFirst: true, autofocus: true)
                        PlateField(
                            label: "Description",
                            text: $description,
                            hint: "Optional",
                            multiline: true
                        )
                    }

                    group("Kind") {
                        PillSelector(
                            options: [ShelfKind.manual, .smart],
                            label: { $0 == .manual ? "Manual" : "Smart" },
                            selection: $kind.animation(Motion.settle)
                        )

                        Text(kind == .manual
                            ? "You choose what goes on it."
                            : "Books join automatically when they match your conditions.")
                            .font(.ui(12.5))
                            .foregroundStyle(palette.ink3Color)
                    }

                    group("Visibility") {
                        PillSelector(
                            options: [ShelfVisibility.private, .public],
                            label: { $0 == .private ? "Private" : "Public" },
                            selection: $visibility
                        )
                    }

                    if kind == .smart { conditions }

                    if let error {
                        Text(error)
                            .font(.ui(13))
                            .foregroundStyle(palette.badColor)
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 48)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .background(ScreenBackground())
            .navigationTitle("New shelf")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") { Task { await create() } }
                        .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || isSaving)
                }
            }
            .onChange(of: rules) { _, _ in Task { await refreshPreview() } }
            .onChange(of: matchMode) { _, _ in Task { await refreshPreview() } }
        }
        .tint(palette.accentColor)
    }

    // Split into named pieces: as one expression this section defeated the
    // type checker outright.
    private var conditions: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Conditions")
            matchSelector
            ruleList
            addConditionButton
            previewLine
        }
    }

    private var matchSelector: some View {
        PillSelector(
            options: [MatchMode.all, .any],
            label: { (mode: MatchMode) in mode == .all ? "Match all" : "Match any" },
            selection: $matchMode
        )
    }

    private var ruleList: some View {
        VStack(spacing: Spacing.sm) {
            ForEach(rules.indices, id: \.self) { (index: Int) in
                RuleEditorRow(rule: $rules[index]) {
                    withAnimation(Motion.settle) { removeRule(at: index) }
                }
            }
        }
    }

    private func removeRule(at index: Int) {
        guard rules.indices.contains(index) else { return }
        rules.remove(at: index)
    }

    private var addConditionButton: some View {
        Button {
            Haptics.tap()
            withAnimation(Motion.settle) {
                rules.append(ShelfRule(field: .tag, op: .is, value: ""))
            }
        } label: {
            Label("Add condition", systemImage: "plus")
                .font(.ui(13.5, weight: .semibold))
                .foregroundStyle(palette.accentColor)
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var previewLine: some View {
        if let preview {
            Text("\(preview.matched) of \(preview.total) books match")
                .font(.monoUI(11))
                .foregroundStyle(palette.ink3Color)
                .contentTransition(.numericText())
                .animation(Motion.snap, value: preview.matched)
        }
    }

    private func group<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel(title)
            content()
        }
    }

    // MARK: - Data

    private func refreshPreview() async {
        guard kind == .smart else { return }
        let valid = rules.filter { !$0.value.trimmingCharacters(in: .whitespaces).isEmpty }
        guard !valid.isEmpty else {
            preview = nil
            return
        }
        preview = try? await UserDataService.previewRule(
            RulePreviewRequest(matchMode: matchMode, rules: valid)
        )
    }

    private func create() async {
        isSaving = true
        error = nil
        defer { isSaving = false }

        let validRules = rules.filter { !$0.value.trimmingCharacters(in: .whitespaces).isEmpty }
        let request = CreateShelfRequest(
            kind: kind,
            name: name,
            description: description.nilIfBlank,
            visibility: visibility,
            matchMode: kind == .smart ? matchMode : nil,
            rules: kind == .smart ? validRules : [],
            bookUUIDs: []
        )
        do {
            _ = try await UserDataService.createShelf(request)
            Haptics.success()
            onCreated()
            dismiss()
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}

/// One smart-shelf condition: field, operator, value.
private struct RuleEditorRow: View {
    @Binding var rule: ShelfRule
    var onDelete: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        Plate {
            VStack(spacing: 0) {
                HStack(spacing: Spacing.sm) {
                    menu(rule.field.label) {
                        ForEach(RuleField.allCases, id: \.self) { field in
                            Button(field.label) {
                                rule.field = field
                                // Keep the operator legal for the new field —
                                // the server rejects a mismatch outright.
                                if !field.acceptedOps.contains(rule.op) {
                                    rule.op = field.acceptedOps.first ?? .is
                                }
                            }
                        }
                    }

                    menu(rule.op.label) {
                        ForEach(rule.field.acceptedOps, id: \.self) { op in
                            Button(op.label) { rule.op = op }
                        }
                    }

                    Spacer(minLength: 0)

                    Button(action: onDelete) {
                        Image(systemName: "minus.circle.fill")
                            .font(.system(size: 17))
                            .foregroundStyle(palette.badColor)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Remove condition")
                }
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 10)

                PlateField(label: "Value", text: $rule.value)
            }
        }
    }

    private func menu<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        Menu {
            content()
        } label: {
            HStack(spacing: 4) {
                Text(title)
                    .font(.ui(13, weight: .medium))
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 9, weight: .semibold))
            }
            .foregroundStyle(palette.ink1Color)
            .padding(.horizontal, 11)
            .padding(.vertical, 7)
            .background(Capsule().fill(palette.bg2Color))
            .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
        }
    }
}
