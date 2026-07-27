//  RefreshTask.swift
//  Pull-to-refresh whose work outlives the redraw it causes.

import SwiftUI

/// Run `action` where the caller's cancellation can't reach it, and wait for it.
///
/// An unstructured `Task` inherits priority and task-locals but not
/// cancellation, and awaiting its value is not itself a cancellation point — so
/// a cancelled caller still gets a completed action rather than an abandoned
/// one. Factored out of [`View.refreshTask`] so the property that matters can be
/// tested without standing up a view.
@MainActor
func runUncancelled(_ action: @escaping @MainActor () async -> Void) async {
    await Task { await action() }.value
}

extension View {
    /// `refreshable`, with the work detached from the action's own cancellation.
    ///
    /// SwiftUI cancels a `refreshable` action the moment the view it is attached
    /// to invalidates. Every refresh here publishes as it goes — the replica
    /// first, the server's answer when it lands — so publishing the replica
    /// cancelled the action, and with it the request still in flight behind it.
    /// The pull then redrew the same stale page: a book added elsewhere never
    /// appeared and a removed one never left, while a filter change picked both
    /// up immediately because its reload runs in a task of its own.
    ///
    /// Awaiting the detached task keeps the spinner up until the refresh has
    /// actually settled. Prefer this to `refreshable` for anything that writes
    /// to state the view reads.
    func refreshTask(_ action: @escaping @MainActor () async -> Void) -> some View {
        refreshable { await runUncancelled(action) }
    }
}
