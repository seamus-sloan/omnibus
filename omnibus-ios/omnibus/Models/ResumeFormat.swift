//  ResumeFormat.swift
//  Which surface a saved position should actually open.
//
//  Shared by the Continue hero, the widget snapshot, and the widget's own
//  deep link, so all three can't disagree about whether a book resumes into
//  the reader or the player.

import Foundation

extension Book {
    /// The format to resume in, given the one a position was recorded in.
    ///
    /// A progress row outlives its file — it soft-references `books.uuid` with
    /// no cascade — so a book whose audiobook has since been removed still
    /// carries an audio position, and offering "Play" for it opens a player
    /// with nothing to play. An empty `formats` means the payload omitted the
    /// list rather than that the book has none, so it defers to the request
    /// instead of overriding it.
    func resumeFormat(for requested: ProgressFormat) -> ProgressFormat {
        guard !formats.isEmpty else { return requested }
        switch requested {
        case .audio: return hasAudiobook ? .audio : .epub
        case .epub: return hasEbook ? .epub : .audio
        }
    }

    /// The same decision for a caller with no opinion — a deep link that named
    /// no format. Reading is the default ask, so a book carrying both resumes
    /// into the reader, and an audio-only one still lands in the player.
    func resumeFormat(for requested: WidgetFormat?) -> ProgressFormat {
        resumeFormat(for: requested.map(ProgressFormat.init) ?? .epub)
    }
}

extension ProgressFormat {
    init(_ format: WidgetFormat) {
        // The two enums share their raw values by contract (`WidgetFormat`'s
        // doc comment), so this cannot fail — but a `!` here would turn a
        // future divergence into a crash on a widget tap.
        self = ProgressFormat(rawValue: format.rawValue) ?? .epub
    }
}

extension WidgetFormat {
    init(_ format: ProgressFormat) {
        self = WidgetFormat(rawValue: format.rawValue) ?? .epub
    }
}
