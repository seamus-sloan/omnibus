//  ReaderIndicators.swift
//  The two centred labels above and below the page, and what each of them says.
//
//  They answer a different question depending on whether the chrome is up.
//  Reading, the questions are "which book am I in" and "which page am I on".
//  With the menu out you are navigating rather than reading, so the chapter's
//  remaining pages and the page count are what's worth the space.

import SwiftUI

/// What the labels above and below the page read, for one reader state.
///
/// Split out of the view so the wording is testable without a rendered
/// hierarchy — four states of `chapterPagesLeft` alone decide the top line.
struct ReaderIndicators {
    let location: RelocateData?
    /// The book's title, which is the top line while reading.
    let title: String
    /// Whether the reader's buttons — the close button and the menu — are up.
    let chromeVisible: Bool

    /// The label above the page.
    ///
    /// Reading, that's the book you're in. With the chrome up it becomes how
    /// much further to a natural place to stop — the question you actually have
    /// mid-book, which a raw page number can't answer. Falls back to the
    /// chapter's own position until epub.js's whole-book locations pass lands.
    ///
    /// Both forms wait on a location so the two labels arrive together rather
    /// than the title hanging over the loading view on its own.
    var top: String? {
        guard let location else { return nil }
        guard chromeVisible else { return title.nilIfBlank }

        let left = location.chapterPagesLeft
        if left > 0 {
            return left == 1
                ? "1 page left in chapter"
                : "\(left) pages left in chapter"
        }
        // `chapter` is 0 when the current href matched no TOC entry (common in
        // front matter) — omit rather than showing a meaningless "Ch. 0".
        guard location.chapter > 0, location.totalChapters > 0 else { return nil }
        return "Chapter \(location.chapter) of \(location.totalChapters)"
    }

    /// The label below the page: which page you're on, and — with the chrome up
    /// — how many there are.
    ///
    /// Page numbers need epub.js's whole-book locations pass, which lands
    /// seconds after the first paint; percent stands in until then rather than
    /// a placeholder that jumps.
    var bottom: String? {
        guard let location else { return nil }
        guard location.hasPageNumbers else {
            return location.pct > 0 ? "\(location.pct)%" : nil
        }
        return chromeVisible
            ? "\(location.page) of \(location.totalPages)"
            : "\(location.page)"
    }
}

/// The two labels, which stay put whether the buttons are showing or not.
///
/// They sit in the bands `#stage` reserves via `env(safe-area-inset-*)`, so
/// they never overlap prose, and they take no touches so the page keeps every
/// gesture. Only the buttons come and go on a centre tap — the position is
/// quiet enough to live there permanently.
struct ReaderIndicatorLabels: View {
    let indicators: ReaderIndicators
    let ink: Color

    var body: some View {
        VStack(spacing: 0) {
            label(indicators.top)
                .padding(.top, Spacing.xs)

            Spacer(minLength: 0)

            label(indicators.bottom)
                .padding(.bottom, Spacing.xs)
        }
        .allowsHitTesting(false)
    }

    /// Given the button's own box rather than its own text height, so a label
    /// sits centred on the line its buttons are on however the type resolves.
    @ViewBuilder
    private func label(_ text: String?) -> some View {
        if let text {
            Text(text)
                .font(.ui(12.5))
                .foregroundStyle(ink.opacity(0.5))
                .lineLimit(1)
                .frame(height: ReaderMenu.buttonSize)
                .padding(.horizontal, 72)
        }
    }
}
