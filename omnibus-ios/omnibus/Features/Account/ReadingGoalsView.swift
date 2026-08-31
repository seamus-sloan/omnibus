//  ReadingGoalsView.swift
//  All three reading targets, in one place under the account.
//
//  They were two sheets hung off the Stats cards, which put the same setting in
//  two places and made a card that *reports* a figure look like a control.
//  Rule 08 test 1 already calls every target here account configuration — set
//  rarely and deliberately, never queued — so this is where they belong, and
//  the Stats cards keep only a pencil that leads here.
//
//  One screen rather than three: the annual goal and the two dailies are read
//  together and set together, and a reader adopting a habit sets more than one
//  at a sitting.

import SwiftUI

/// Latest annual target the write path accepts — `MAX_GOAL_TARGET`.
private let maxAnnualTarget: Int64 = 10_000

/// What a field's text means as a target.
///
/// Its own type rather than a `Result`, whose failure has to be an `Error` —
/// this failure is a sentence to print under a field, not something to throw.
enum GoalTargetInput: Equatable {
    /// A target to write. `nil` is the clear.
    case target(Int64?)
    /// The message to print under the field.
    case invalid(String)
}

struct ReadingGoalsView: View {
    @Environment(\.palette) private var palette

    @State private var summary: StatsSummary?
    @State private var isLoading = true
    @State private var loadError: String?
    @State private var annualDraft = ""
    @State private var dailyDrafts: [DailyGoalKind: String] = [:]
    /// Per-kind, because the write path is per-kind: one failed PUT must not
    /// read as though the others failed too.
    @State private var errors: [String: String] = [:]
    @State private var isSaving = false
    @State private var savedCount = 0
    /// Whether the drafts have been filled from a summary at least once.
    ///
    /// Its own flag rather than "the drafts are still empty": once a summary
    /// lands, an unseeded blank draft *is* a change against a stored target,
    /// so the mid-edit guard below would read the very first fill as an edit
    /// and refuse to make it — which is how this screen first opened blank
    /// with every field flagged as modified.
    @State private var hasSeeded = false

    private var isOnline: Bool { Connectivity.shared.isOnline }

    /// The calendar year the annual goal belongs to, taken from the server's
    /// `asOfDay` rather than the device clock so this screen and the card that
    /// leads to it can never straddle different years.
    private var year: String {
        let day = summary?.asOfDay ?? ""
        return day.count >= 4 ? String(day.prefix(4)) : ""
    }

    var body: some View {
        Group {
            if isLoading && summary == nil {
                LoadingView()
            } else if let loadError, summary == nil {
                ErrorStateView(message: loadError) { Task { await load() } }
            } else {
                form
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Reading goals")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
    }

    private var form: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 28) {
                annual
                daily
                saveButton
                if savedCount > 0, errors.isEmpty {
                    statusLine("Saved.", isError: false)
                }
                if !isOnline {
                    statusLine(
                        "You're offline. Goals are saved on the server, so this can wait.",
                        isError: false)
                }
            }
            .screenPadding()
            .padding(.top, Spacing.md)
            .padding(.bottom, 40)
        }
        .scrollIndicators(.hidden)
        .scrollDismissesKeyboard(.interactively)
    }

    // MARK: - Sections

    private var annual: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("This year")

            Plate {
                PlateField(
                    label: year.isEmpty ? "Books this year" : "Books in \(year)",
                    text: $annualDraft,
                    isEdited: annualDraft != storedAnnual,
                    isFirst: true,
                    hint: "1–\(grouped(maxAnnualTarget))",
                    keyboard: .numberPad
                )
            }

            caption(
                errors["annual"]
                    ?? "Counts distinct books finished inside the year. Leave blank for no goal.",
                isError: errors["annual"] != nil
            )
        }
    }

    private var daily: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Every day")

            Plate {
                ForEach(Array(DailyGoalKind.allCases.enumerated()), id: \.element) { index, kind in
                    PlateField(
                        label: kind.label,
                        text: binding(for: kind),
                        isEdited: (dailyDrafts[kind] ?? "") != stored(kind),
                        isFirst: index == 0,
                        hint: "1–\(grouped(kind.maxTarget))",
                        keyboard: .numberPad
                    )
                }
            }

            // One line per kind, so a bounced pages target doesn't look like a
            // problem with the minutes one beside it.
            ForEach(DailyGoalKind.allCases) { kind in
                if let message = errors[kind.rawValue] {
                    caption(message, isError: true)
                }
            }

            caption(
                "Daily goals recur — a target stands until you change it, and today's progress is "
                    + "counted against it from the moment it's set. Leave blank for no goal.",
                isError: false
            )
        }
    }

    private var canSave: Bool { !isSaving && isOnline && hasChanges }

    private var saveButton: some View {
        Button {
            Task { await save() }
        } label: {
            Text(isSaving ? "Saving…" : "Save")
        }
        .buttonStyle(FilledButtonStyle())
        .disabled(!canSave)
        // `FilledButtonStyle` draws the accent ground whatever the button's
        // state, so without this a Save with nothing to save looks live and
        // does nothing when pressed.
        .opacity(canSave ? 1 : 0.4)
        .animation(Motion.snap, value: canSave)
    }

    private func caption(_ text: String, isError: Bool) -> some View {
        Text(text)
            .font(.ui(12))
            .foregroundStyle(isError ? palette.badColor : palette.ink3Color)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func statusLine(_ text: String, isError: Bool) -> some View {
        Text(text)
            .font(.ui(13))
            .foregroundStyle(isError ? palette.badColor : palette.ink2Color)
            .fixedSize(horizontal: false, vertical: true)
    }

    // MARK: - Drafts

    private func binding(for kind: DailyGoalKind) -> Binding<String> {
        Binding(
            get: { dailyDrafts[kind] ?? "" },
            set: { dailyDrafts[kind] = $0 }
        )
    }

    private var storedAnnual: String { summary?.goal.map { "\($0.target)" } ?? "" }

    private func stored(_ kind: DailyGoalKind) -> String {
        summary?.dailyGoals[kind].map { "\($0.target)" } ?? ""
    }

    private var hasChanges: Bool {
        annualDraft.trimmed != storedAnnual
            || DailyGoalKind.allCases.contains { (dailyDrafts[$0] ?? "").trimmed != stored($0) }
    }

    private func seedDrafts() {
        annualDraft = storedAnnual
        for kind in DailyGoalKind.allCases {
            dailyDrafts[kind] = stored(kind)
        }
        hasSeeded = true
    }

    // MARK: - Reading and writing

    private func load() async {
        do {
            // Any range carries the same goals — both are unwindowed — and
            // Month is the range the Stats tab opens on, so this is usually
            // already in the replica.
            for try await read in UserDataService.stats(range: .month) {
                summary = read.value
                loadError = nil
                isLoading = false
                // The first summary always seeds. After that, only while the
                // reader has not started typing — a live refresh landing
                // mid-edit must not overwrite what they entered.
                if !hasSeeded || !hasChanges { seedDrafts() }
            }
        } catch {
            if summary == nil {
                loadError = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
        isLoading = false
    }

    /// Writes only what moved, one request per target.
    ///
    /// `DailyGoalUpdate` is per-kind and the annual goal has its own route, so
    /// setting all three is three requests. Each keeps its own error and the
    /// ones that landed stay landed — a partial failure must not read as a
    /// failure of the whole form.
    private func save() async {
        errors = [:]
        savedCount = 0
        isSaving = true
        defer { isSaving = false }

        await saveAnnual()
        for kind in DailyGoalKind.allCases {
            await saveDaily(kind)
        }
    }

    private func saveAnnual() async {
        guard annualDraft.trimmed != storedAnnual else { return }
        switch Self.target(from: annualDraft, max: maxAnnualTarget, unit: "book") {
        case let .target(value): await writeAnnual(value)
        case let .invalid(message): errors["annual"] = message
        }
    }

    /// What a field's text means as a target: a number inside the kind's
    /// bounds, `nil` for a clear, or the message to print under the field.
    ///
    /// **A blank field is the clear**, which is why there is no Clear button:
    /// the form has one explicit Save, so emptying a target and pressing it is
    /// as deliberate as typing one. The bounds are the server's own, re-checked
    /// here so an out-of-range target is named under the field rather than
    /// bounced as a 400 with nothing to point at.
    static func target(from raw: String, max: Int64, unit: String) -> GoalTargetInput {
        let trimmed = raw.trimmed
        guard !trimmed.isEmpty else { return .target(nil) }
        guard let value = Int64(trimmed), (1...max).contains(value) else {
            let bound = NumberFormatter.localizedString(
                from: NSNumber(value: max), number: .decimal)
            return .invalid("Enter a whole number of \(unit)s between 1 and \(bound).")
        }
        return .target(value)
    }

    private func writeAnnual(_ value: Int64?) async {
        do {
            summary?.goal = try await UserDataService.setReadingGoal(target: value)
            savedCount += 1
        } catch {
            errors["annual"] = message(error)
        }
    }

    private func saveDaily(_ kind: DailyGoalKind) async {
        let raw = dailyDrafts[kind] ?? ""
        guard raw.trimmed != stored(kind) else { return }

        let target: Int64?
        switch Self.target(from: raw, max: kind.maxTarget, unit: kind.unit) {
        case let .target(value): target = value
        case let .invalid(message):
            errors[kind.rawValue] = message
            return
        }

        do {
            summary?.dailyGoals = try await UserDataService.setDailyGoal(
                kind: kind, target: target)
            savedCount += 1
        } catch {
            errors[kind.rawValue] = message(error)
        }
    }

    private func message(_ error: Error) -> String {
        (error as? APIError)?.errorDescription ?? error.localizedDescription
    }

    private func grouped(_ n: Int64) -> String {
        NumberFormatter.localizedString(from: NSNumber(value: n), number: .decimal)
    }
}

extension String {
    fileprivate var trimmed: String { trimmingCharacters(in: .whitespaces) }
}
