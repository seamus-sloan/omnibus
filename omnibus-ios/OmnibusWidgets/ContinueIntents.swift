//  ContinueIntents.swift
//  Moving the one-book card along the rail without leaving the Home Screen.
//
//  A widget cannot be swiped — the system renders it as a snapshot, and the
//  only interaction it offers is a tap: a `widgetURL`/`Link` that opens the
//  app, or an `AppIntent` that runs in the extension. So the card flips rather
//  than scrolls, and this is the flip.

import AppIntents
import WidgetKit

struct ShowNextBook: AppIntent {
    static let title: LocalizedStringResource = "Show the next book"
    static let description = IntentDescription(
        "Moves the Continue widget on to the next book you have in progress."
    )

    /// The whole point is that the rail can be walked from the Home Screen.
    /// Launching the app to answer a tap would make the control slower than
    /// just opening the book it is trying to skip past.
    static let openAppWhenRun = false

    func perform() async throws -> some IntentResult {
        let snapshot = WidgetStore.load() ?? .empty()
        WidgetStore.setCursor(snapshot.next(after: WidgetStore.cursor())?.id)
        // WidgetKit reloads after an intent on its own, but only for the
        // widget whose button was tapped. Naming the kind refreshes a second
        // Continue widget on the same screen, which is otherwise left showing
        // a cursor that has already moved.
        WidgetCenter.shared.reloadTimelines(ofKind: WidgetKind.continueReading)
        return .result()
    }
}
