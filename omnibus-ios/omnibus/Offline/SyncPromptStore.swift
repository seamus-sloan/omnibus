//  SyncPromptStore.swift
//  Dismissal memory for the cross-format resume prompts: the newest source
//  clock the user has declined, per (book, target). Account-scoped in the
//  offline kv store — wiped on account switch. The web client keeps the
//  same memory in localStorage under its own key shape; only the re-arm
//  rule is shared, not the literal key.

import Foundation

enum SyncPromptStore {
    static func key(uuid: String, target: ProgressFormat) -> String {
        "syncprompt:\(uuid):\(target.rawValue)"
    }

    static func dismissedClock(uuid: String, target: ProgressFormat) async -> Int64? {
        guard let data = await OfflineStore.shared.cacheGet(key(uuid: uuid, target: target)),
              let text = String(data: data, encoding: .utf8)
        else { return nil }
        return Int64(text)
    }

    static func storeDismissedClock(_ clock: Int64, uuid: String, target: ProgressFormat) async {
        await OfflineStore.shared.cachePut(
            key(uuid: uuid, target: target),
            Data("\(clock)".utf8)
        )
    }

    /// Re-arm rule (pure): a declined source clock stays quiet until the
    /// other format's position advances past it.
    static func shouldOffer(sourceClock: Int64, dismissed: Int64?) -> Bool {
        guard let dismissed else { return true }
        return sourceClock > dismissed
    }

    /// Whether the candidate's mapped file is the one being played: an
    /// explicit selection must match exactly; no selection means the
    /// default (first-by-ordinal) file is loaded. A candidate with no file
    /// is never seekable here.
    static func fileMatches(selected: Int64?, defaultID: Int64?, candidate: Int64?) -> Bool {
        guard let candidate else { return false }
        if let selected { return selected == candidate }
        return defaultID == candidate
    }
}
