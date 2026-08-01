//  ComicPosition.swift
//  How a comic page maps onto the shared progress record.
//
//  Mirror of `omnibus_shared::{comic_page_anchor, parse_comic_page_anchor}`
//  and the web pager's percent/start-page math. Comics reuse the Epub-format
//  progress row: the `epub_cfi` slot holds a self-describing `comic-page:N`
//  anchor and `progress_percent` carries the cross-surface half, so Continue
//  Reading and cross-device sync work unchanged. Every client round-trips
//  positions through these helpers rather than inventing its own encoding.

import Foundation

enum ComicPosition {
    private static let anchorPrefix = "comic-page:"

    /// Position anchor for a 0-based comic page, stored in the `epub_cfi`
    /// slot of an Epub-format progress row.
    static func anchor(page: Int) -> String {
        "\(anchorPrefix)\(page)"
    }

    /// Parse an [`anchor(page:)`] back to its 0-based page index. `nil` for
    /// anything else — including a real EPUB CFI — so callers can fall back
    /// to `progress_percent` without misreading a foreign position.
    static func parseAnchor(_ anchor: String) -> Int? {
        guard anchor.hasPrefix(anchorPrefix) else { return nil }
        return Int(anchor.dropFirst(anchorPrefix.count))
    }

    /// Whole-book percent for a 0-based `page` of `count` — the
    /// cross-surface half of the saved position. Last page reads as 100.
    static func percent(page: Int, count: Int) -> Int64 {
        guard count > 0 else { return 0 }
        return Int64((Double((page + 1) * 100) / Double(count)).rounded())
    }

    /// The 0-based page a saved record resumes at: the exact anchor when
    /// present, else the inverse of [`percent(page:count:)`], else page 0.
    /// Clamped so a shrunken re-scan of the archive can't land out of range.
    static func startPage(anchor: String?, percent: Int64?, count: Int) -> Int {
        guard count > 0 else { return 0 }
        let last = count - 1
        if let anchor, let page = parseAnchor(anchor) {
            return min(page, last)
        }
        if let percent {
            let clamped = Double(min(max(percent, 0), 100))
            let approx = Int((clamped / 100 * Double(count)).rounded())
            return min(max(approx - 1, 0), last)
        }
        return 0
    }
}
