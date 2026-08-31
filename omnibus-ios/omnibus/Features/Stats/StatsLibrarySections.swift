//  StatsLibrarySections.swift
//  The library's own scale and make-up, and the session log beneath them.
//
//  All three are standing sections: `LibrarySize` and `LibraryComposition` are
//  library-scoped and deliberately off `StatsSummary` so a period switch never
//  recomputes them, and the log is its own keyset-paged read.

import SwiftUI

// MARK: - Derivations
//
// Kept on `StatsView` rather than moved onto the views below: they are the
// units and the empty-state rules, shared with the web card's own helpers and
// asserted directly by `StatsCodecTests`.

extension StatsView {
    /// The five panels, in the order they read: what the files are, then what
    /// the books are. Mirrors the web card's `build_panels`.
    static func compositionPanels(_ c: LibraryComposition) -> [CompositionPanel] {
        [
            CompositionPanel(
                title: "Formats", dimension: c.formats,
                // Coverage is always the whole library here (a live book has a
                // file by definition), so the useful disclosure is the overlap.
                note: overlapNote(c.formats),
                empty: "No files indexed yet."),
            CompositionPanel(
                title: "Languages", dimension: c.languages,
                note: coverageNote(c.languages, of: c.books),
                empty: "No language metadata yet."),
            CompositionPanel(
                title: "Publishers", dimension: c.publishers,
                note: coverageNote(c.publishers, of: c.books),
                empty: "No publisher metadata yet."),
            CompositionPanel(
                title: "Published", dimension: c.decades,
                // The uncovered books here are the ones with an absent or
                // unparseable pubdate — unknown, never bucketed into a decade.
                note: coverageNote(c.decades, of: c.books),
                empty: "No publication dates yet."),
            CompositionPanel(
                title: "Genres", dimension: c.genres,
                note: "hand-assigned \u{2014} " + coverageNote(c.genres, of: c.books),
                empty: "No genres assigned yet."),
        ]
    }

    /// "across 58 of 1,510 books" — the denominator, always. A distribution
    /// without its coverage is a guess wearing a chart.
    static func coverageNote(_ dimension: CompositionDimension, of libraryBooks: Int64) -> String {
        coverageLabel(dimension.coverage, of: libraryBooks)
    }

    /// How many books are held in more than one format, and so counted in more
    /// than one bar. Without it the bars simply don't add up to the library.
    static func overlapNote(_ dimension: CompositionDimension) -> String? {
        let overlap = dimension.overlap
        guard overlap > 0 else { return nil }
        return "+\(overlap) \(overlap == 1 ? "book" : "books") held in more than one format"
    }

    /// The footnote for `books` rows whose files are gone. They carry no
    /// format, so they'd otherwise vanish from the bars and leave the counts
    /// failing to reconcile against the library.
    static func ghostedNote(_ ghosted: Int64) -> String? {
        guard ghosted > 0 else { return nil }
        return
            "\(ghosted) \(ghosted == 1 ? "book" : "books") excluded \u{2014} indexed once, no files on disk now"
    }

    /// The library figures worth rendering, skipping anything nothing has
    /// been measured for — a "0 words" row describes a library that doesn't
    /// exist. Mirrors the web card's `build_figures`.
    static func libraryFigures(_ size: LibrarySize) -> [LibraryFigure] {
        var figures: [LibraryFigure] = []
        if !size.words.isEmpty {
            figures.append(
                LibraryFigure(
                    value: compactCount(size.words.total),
                    unit: "words",
                    coverage: coverageLabel(size.words, of: size.books)
                ))
        }
        if !size.pages.isEmpty {
            figures.append(
                LibraryFigure(
                    value: compactCount(size.pages.total),
                    unit: "est. pages",
                    coverage: coverageLabel(size.pages, of: size.books)
                ))
        }
        if !size.listeningSeconds.isEmpty {
            let (value, unit) = audioValue(size.listeningSeconds.total)
            figures.append(
                LibraryFigure(
                    value: value,
                    unit: unit,
                    coverage: coverageLabel(size.listeningSeconds, of: size.books)
                ))
        }
        return figures
    }

    /// A large count in the form a reader can hold — "412M", "1.6M", "94.2K",
    /// "812". Nobody needs the last four digits of a word count.
    static func compactCount(_ n: Int64) -> String {
        let v = Double(n)
        // Each tier opens at 999.5 of the one below rather than at a clean
        // power of ten: 999_999 rounds to 1000 at "K", so it has to render as
        // "1.0M". Mirrors `compact` in frontend/src/pages/stats/library.rs.
        for (limit, div, suffix) in [(999.5e6, 1e9, "B"), (999.5e3, 1e6, "M"), (1e4, 1e3, "K")] {
            if v >= limit {
                let scaled = v / div
                return String(format: scaled < 100 ? "%.1f\(suffix)" : "%.0f\(suffix)", scaled)
            }
        }
        return "\(n)"
    }

    /// Audio length in the unit that fits it: hours below a week, days beyond.
    /// "94 days of audio" is the sentence this section exists to let a reader
    /// say; 2,256 hours is the same fact nobody can picture.
    static func audioValue(_ seconds: Int64) -> (String, String) {
        let hours = Double(seconds) / 3600
        // Round first, then pick the unit off the rounded figure: branching on
        // the raw hours renders 1h40m as "2 hour", and promotes to days only
        // after the hours reading has already rounded to 168.
        let wholeHours = Int64(hours.rounded())
        if wholeHours < 168 {
            return ("\(wholeHours)", wholeHours == 1 ? "hour" : "hours")
        }
        return (String(format: "%.0f", hours / 24), "days")
    }

    /// "across 1,204 of 1,510 books" — the denominator, always. A figure
    /// without it is a guess wearing a number.
    static func coverageLabel(_ measured: MeasuredTotal, of libraryBooks: Int64) -> String {
        "across \(groupedCount(measured.books)) of \(groupedCount(libraryBooks)) books"
    }

    /// A count with its thousands separators. Shared so a bar's own number and
    /// the coverage line beneath it can't render the same figure two ways.
    static func groupedCount(_ n: Int64) -> String {
        NumberFormatter.localizedString(from: NSNumber(value: n), number: .decimal)
    }
}

// MARK: - Library size

/// One library-scale figure: the total, its unit, and the coverage behind it.
struct LibraryFigure: Identifiable, Hashable {
    let value: String
    let unit: String
    let coverage: String

    var id: String { unit }
}

struct LibrarySizeSection: View {
    let size: LibrarySize

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(StatsView.libraryFigures(size)) { figure in
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(figure.value)
                            .font(.display(28, weight: .semibold))
                            .foregroundStyle(palette.ink0Color)
                        Text(figure.unit)
                            .font(.ui(13))
                            .foregroundStyle(palette.ink2Color)
                    }
                    Text(figure.coverage)
                        .font(.monoUI(11))
                        .foregroundStyle(palette.ink3Color)
                }
                .accessibilityElement(children: .combine)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - Library composition

/// One rendered dimension: its heading, its bars, and the line beneath them
/// that says what the bars can't speak for.
struct CompositionPanel: Identifiable, Hashable {
    let title: String
    let dimension: CompositionDimension
    let note: String?
    let empty: String

    var id: String { title }
}

struct LibraryCompositionSection: View {
    let composition: LibraryComposition

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            ForEach(StatsView.compositionPanels(composition)) { panel in
                CompositionPanelView(panel: panel)
            }
            if let ghosted = StatsView.ghostedNote(composition.ghostedBooks) {
                Text(ghosted)
                    .font(.monoUI(11))
                    .foregroundStyle(palette.ink3Color)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// One dimension's bars, or its empty state. A dimension nothing in the
/// library carries renders a sentence rather than an axis with no bars on it.
private struct CompositionPanelView: View {
    let panel: CompositionPanel

    @Environment(\.palette) private var palette

    /// Scaled to the tallest bar rather than to the library total: a histogram
    /// whose bars are all four points wide has drawn the shape out of itself.
    private var peak: Int64 { panel.dimension.slices.map(\.books).max() ?? 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(panel.title)
                .font(.display(15))
                .foregroundStyle(palette.ink1Color)
            if panel.dimension.slices.isEmpty {
                Text(panel.empty)
                    .font(.ui(13))
                    .foregroundStyle(palette.ink3Color)
            } else {
                ForEach(panel.dimension.slices) { slice in
                    CompositionBar(slice: slice, peak: peak)
                }
                if let note = panel.note {
                    Text(note)
                        .font(.monoUI(11))
                        .foregroundStyle(palette.ink3Color)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct CompositionBar: View {
    let slice: CompositionSlice
    let peak: Int64

    @Environment(\.palette) private var palette

    private var fraction: Double {
        guard peak > 0 else { return 0 }
        return min(1, Double(slice.books) / Double(peak))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(slice.label)
                    .font(.ui(14))
                    .foregroundStyle(palette.ink1Color)
                    .lineLimit(1)
                Spacer(minLength: Spacing.sm)
                // The count, not the share: "48 books" answers the question a
                // reader brought to a composition chart. Grouped like the
                // coverage line, so a four-digit bucket doesn't read
                // differently from its own note.
                Text(StatsView.groupedCount(slice.books))
                    .font(.monoUI(12))
                    .foregroundStyle(palette.ink2Color)
            }
            StatsBar(fraction: fraction, height: 8)
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Session log

/// One sitting: when it started, what it was, and how long it ran.
struct SessionLogRow: View {
    let entry: SessionLogEntry

    @Environment(\.palette) private var palette

    /// Device-local, unlike the web log's UTC: this is a native app on a
    /// reader's own phone, and a sitting they remember starting at 9pm should
    /// say so.
    private var when: String {
        Date(timeIntervalSince1970: TimeInterval(entry.startedAt))
            .formatted(date: .abbreviated, time: .shortened)
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(entry.title)
                    .font(.ui(13.5, weight: .medium))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)
                Text("\(when) \u{b7} \(entry.format.label)")
                    .font(.monoUI(10))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(Format.humanDuration(entry.seconds))
                .font(.ui(12.5))
                .foregroundStyle(palette.ink2Color)
        }
        .padding(.vertical, 9)
        .overlay(alignment: .top) { Hairline() }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}
