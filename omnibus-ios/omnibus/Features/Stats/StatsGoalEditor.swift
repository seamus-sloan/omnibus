//  StatsGoalEditor.swift
//  Setting, changing and clearing the reading goals.
//
//  Both goal cards open this. Rule 08 test 1 puts every target here in the
//  account-configuration tier: direct calls that surface their own failure,
//  never queued ops, and Save is disabled while offline rather than failing
//  after the fact.

import SwiftUI

/// Which card opened the editor.
enum GoalEditorTarget: Identifiable, Hashable {
    /// Today's pages and minutes — two independent targets on one sheet.
    case daily
    /// The books-this-year target, for the named calendar year.
    case annual(year: String)

    var id: String {
        switch self {
        case .daily: "daily"
        case .annual(let year): "annual-\(year)"
        }
    }

    var title: String {
        switch self {
        case .daily: "Daily goals"
        case .annual(let year): "\(year) reading goal"
        }
    }
}

/// Latest annual target the write path accepts — `MAX_GOAL_TARGET`.
private let maxAnnualTarget: Int64 = 10_000

struct GoalEditorSheet: View {
    let target: GoalEditorTarget
    let summary: StatsSummary
    /// Handed the server's answer so the tab can fold it straight into the
    /// rendered summary rather than waiting on a reload.
    let onDailySaved: (DailyGoals) -> Void
    let onAnnualSaved: (ReadingGoal?) -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.palette) private var palette

    @State private var drafts: [String: String] = [:]
    /// Per-kind, because the write path is per-kind: one failed PUT must not
    /// read as though the other one failed too.
    @State private var errors: [String: String] = [:]
    @State private var isSaving = false

    private var isOnline: Bool { Connectivity.shared.isOnline }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.xl) {
                    switch target {
                    case .daily:
                        ForEach(DailyGoalKind.allCases) { kind in
                            dailyField(kind)
                        }
                        Text(
                            "Daily goals recur — a target stands until you change it, and today's "
                                + "progress is counted against it from the moment it's set."
                        )
                        .font(.ui(12))
                        .foregroundStyle(palette.ink3Color)
                        .fixedSize(horizontal: false, vertical: true)
                    case .annual(let year):
                        annualField(year)
                    }

                    if !isOnline {
                        Text("You're offline. Goals are saved on the server, so this can wait.")
                            .font(.ui(12.5))
                            .foregroundStyle(palette.ink2Color)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.lg)
            }
            .background(ScreenBackground())
            .navigationTitle(target.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isSaving ? "Saving\u{2026}" : "Save") { Task { await save() } }
                        .disabled(isSaving || !isOnline)
                }
            }
        }
        .onAppear(perform: seedDrafts)
    }

    // MARK: - Fields

    private func dailyField(_ kind: DailyGoalKind) -> some View {
        field(
            key: kind.rawValue,
            label: kind.label,
            placeholder: "No target",
            hint: "1 to \(grouped(kind.maxTarget)) \(kind.unit)s",
            hasTarget: summary.dailyGoals[kind] != nil
        ) {
            await clearDaily(kind)
        }
    }

    private func annualField(_ year: String) -> some View {
        field(
            key: "annual",
            label: "Books to finish in \(year)",
            placeholder: "No target",
            hint: "1 to \(grouped(maxAnnualTarget)) books",
            hasTarget: summary.goal != nil
        ) {
            await writeAnnual(nil)
        }
    }

    /// One target's row: its number field, its bounds, and a Clear that is
    /// shown **only when a target exists** — mirroring the web editor's
    /// `is_some()` guard, since clearing nothing is not an action.
    private func field(
        key: String,
        label: String,
        placeholder: String,
        hint: String,
        hasTarget: Bool,
        clear: @escaping () async -> Void
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(label)
                .font(.ui(13.5, weight: .medium))
                .foregroundStyle(palette.ink1Color)

            HStack(spacing: Spacing.md) {
                TextField(
                    placeholder,
                    text: Binding(
                        get: { drafts[key] ?? "" },
                        set: { drafts[key] = $0 }
                    )
                )
                .keyboardType(.numberPad)
                .font(.monoUI(17))
                .foregroundStyle(palette.ink0Color)
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .background(
                    RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                        .fill(palette.bg1Color)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                        .strokeBorder(
                            errors[key] == nil ? palette.line2.color : palette.badColor,
                            lineWidth: errors[key] == nil ? 0.5 : 1
                        )
                )

                if hasTarget {
                    Button("Clear") { Task { await clear() } }
                        .buttonStyle(QuietButtonStyle())
                        .disabled(isSaving || !isOnline)
                }
            }

            Text(errors[key] ?? hint)
                .font(.ui(12))
                .foregroundStyle(errors[key] == nil ? palette.ink3Color : palette.badColor)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: - Writing

    private func seedDrafts() {
        switch target {
        case .daily:
            for kind in DailyGoalKind.allCases {
                drafts[kind.rawValue] = summary.dailyGoals[kind].map { "\($0.target)" } ?? ""
            }
        case .annual:
            drafts["annual"] = summary.goal.map { "\($0.target)" } ?? ""
        }
    }

    private func save() async {
        errors = [:]
        isSaving = true
        defer { isSaving = false }

        switch target {
        case .daily: await saveDaily()
        case .annual: await saveAnnual()
        }
        // Held open on any failure so the reader can see which kind bounced
        // and try again, rather than watching the sheet close on an error.
        if errors.isEmpty { dismiss() }
    }

    /// One PUT per kind, in order, because `DailyGoalUpdate` is per-kind.
    ///
    /// Kinds are skipped when the draft matches what is already stored, so a
    /// reader who came in to change one target doesn't re-write the other; and
    /// a kind that fails leaves the ones that already landed alone.
    private func saveDaily() async {
        for kind in DailyGoalKind.allCases {
            let raw = (drafts[kind.rawValue] ?? "").trimmingCharacters(in: .whitespaces)
            let stored = summary.dailyGoals[kind]?.target

            // An emptied field is not a clear: Clear is its own control, and
            // treating a blank as one would drop a target on a slip of the
            // keyboard.
            guard !raw.isEmpty else { continue }
            guard let value = Int64(raw), (1...kind.maxTarget).contains(value) else {
                errors[kind.rawValue] =
                    "Enter a whole number of \(kind.unit)s between 1 and \(grouped(kind.maxTarget))."
                continue
            }
            guard value != stored else { continue }

            do {
                onDailySaved(try await UserDataService.setDailyGoal(kind: kind, target: value))
            } catch {
                errors[kind.rawValue] = message(error)
            }
        }
    }

    private func clearDaily(_ kind: DailyGoalKind) async {
        isSaving = true
        defer { isSaving = false }
        do {
            onDailySaved(try await UserDataService.setDailyGoal(kind: kind, target: nil))
            drafts[kind.rawValue] = ""
            errors[kind.rawValue] = nil
        } catch {
            errors[kind.rawValue] = message(error)
        }
    }

    private func saveAnnual() async {
        let raw = (drafts["annual"] ?? "").trimmingCharacters(in: .whitespaces)
        guard !raw.isEmpty else { return }
        guard let value = Int64(raw), (1...maxAnnualTarget).contains(value) else {
            errors["annual"] =
                "Enter a whole number of books between 1 and \(grouped(maxAnnualTarget))."
            return
        }
        guard value != summary.goal?.target else { return }
        await writeAnnual(value)
    }

    private func writeAnnual(_ value: Int64?) async {
        isSaving = true
        defer { isSaving = false }
        do {
            onAnnualSaved(try await UserDataService.setReadingGoal(target: value))
            drafts["annual"] = value.map { "\($0)" } ?? ""
            errors["annual"] = nil
            if value == nil { dismiss() }
        } catch {
            errors["annual"] = message(error)
        }
    }

    private func message(_ error: Error) -> String {
        (error as? APIError)?.errorDescription ?? error.localizedDescription
    }

    private func grouped(_ n: Int64) -> String {
        NumberFormatter.localizedString(from: NSNumber(value: n), number: .decimal)
    }
}
