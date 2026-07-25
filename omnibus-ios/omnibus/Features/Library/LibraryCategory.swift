//  LibraryCategory.swift
//  The header category strip's buckets.

import Foundation

enum LibraryCategory: Hashable, CaseIterable {
    case all, ebooks, audiobooks, downloaded

    var label: String {
        switch self {
        case .all: "All"
        case .ebooks: "Ebooks"
        case .audiobooks: "Audiobooks"
        case .downloaded: "Offline"
        }
    }

    var icon: String {
        switch self {
        case .all: "square.grid.2x2"
        case .ebooks: "book"
        case .audiobooks: "headphones"
        case .downloaded: "arrow.down.circle"
        }
    }

    /// Formats pushed to `GET /api/ebooks?formats=`. Empty means no server-side
    /// filter — `downloaded` has no server equivalent and is applied client-side
    /// against the download registry.
    var serverFormats: [String] {
        switch self {
        case .all, .downloaded: []
        case .ebooks: ["epub", "pdf", "mobi", "azw3", "cbz", "cbr"]
        case .audiobooks: ["m4b", "m4a", "mp3", "aac", "flac", "ogg", "opus"]
        }
    }

    static var strip: [CategoryStrip<LibraryCategory>.Item] {
        allCases.map { .init(id: $0, label: $0.label, icon: $0.icon) }
    }
}
