//  ContinueWidgetView.swift
//  One card per family, plus the three ways the card can be empty.
//
//  The families are not the same layout at three sizes: small answers "what am
//  I in the middle of", medium "which of my two or three", large "and when did
//  I last touch each". Each is sized around the one question it answers.

import SwiftUI
import WidgetKit

struct ContinueWidgetView: View {
    let snapshot: WidgetSnapshot

    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var scheme

    /// The whole card takes its colour from whichever book leads it, the way
    /// the book-detail hero takes its wash from the book it is about.
    private var theme: WidgetTheme {
        WidgetTheme(tone: snapshot.books.first?.tone, scheme: scheme)
    }

    var body: some View {
        content
            .padding(Layout.margin)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .containerBackground(for: .widget) { theme.ground }
    }

    @ViewBuilder
    private var content: some View {
        if snapshot.books.isEmpty {
            EmptyCard(state: snapshot.state, theme: theme, family: family)
        } else {
            switch family {
            case .systemSmall: SmallCard(book: snapshot.books[0], theme: theme)
            case .systemLarge: LargeCard(books: snapshot.books, theme: theme)
            default: MediumCard(books: snapshot.books, theme: theme)
            }
        }
    }
}

private enum Layout {
    static let margin: CGFloat = 14
    /// The most a `systemMedium` fits across without the titles becoming two
    /// truncated words each.
    static let mediumColumns = 3
}

// MARK: - Small

/// One book, given the whole card: cover, title, and where you are in it.
///
/// The sizes here are a budget, not a preference. A `systemSmall` is 134pt of
/// content on a 6.3" phone, and the cover, the two spacings, and the footer are
/// all fixed — so whatever they don't claim is what the title gets, and
/// `layoutPriority` cannot conjure more. At a 74pt cover and 8pt spacings the
/// title was left 15pt, which is one line, so every title longer than the card
/// is wide truncated mid-word. Sized to fit two lines instead: nothing on this
/// card matters more than which book it is.
private struct SmallCard: View {
    let book: WidgetBook
    let theme: WidgetTheme

    var body: some View {
        // Centred as a column. A 2:3 cover is far narrower than the card, so
        // ranged left it sat off in one corner — and centring the cover over a
        // left-ranged title just moves the mismatch onto the type.
        VStack(alignment: .center, spacing: 6) {
            WidgetCover(book: book, theme: theme, cornerRadius: 5)
                .frame(height: 60)
                .shadow(color: .black.opacity(0.3), radius: 6, y: 3)
                // Also what holds the column at the card's full width: with no
                // bar to draw (a CFI-only EPUB save has no percentage) nothing
                // else here is greedy, and the stack would shrink to the
                // title's width and be pinned to the leading edge — centred
                // within itself, off-centre on the card.
                .frame(maxWidth: .infinity)

            Text(book.title)
                .font(.system(size: 13, weight: .semibold, design: .serif))
                .foregroundStyle(theme.ink0)
                .lineLimit(2)
                .multilineTextAlignment(.center)
                // Claims its two lines ahead of the spacer below it.
                .layoutPriority(1)

            Spacer(minLength: 0)

            PositionFooter(book: book, theme: theme)
        }
        .widgetURL(book.deepLink)
    }
}

// MARK: - Medium

/// Three across — the shape of "which of the two or three am I picking up".
private struct MediumCard: View {
    let books: [WidgetBook]
    let theme: WidgetTheme

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ForEach(books.prefix(Layout.mediumColumns)) { book in
                Link(destination: book.deepLink) {
                    // Centred as a column, for the same reason as the small
                    // card's — see `SmallCard`.
                    VStack(alignment: .center, spacing: 6) {
                        WidgetCover(book: book, theme: theme)
                            .frame(height: 80)
                            .shadow(color: .black.opacity(0.28), radius: 5, y: 3)
                            .frame(maxWidth: .infinity)

                        Text(book.title)
                            .font(.system(size: 11.5, weight: .semibold, design: .serif))
                            .foregroundStyle(theme.ink0)
                            .lineLimit(2)
                            .multilineTextAlignment(.center)

                        Spacer(minLength: 0)

                        if let fraction = book.fraction {
                            WidgetProgressBar(fraction: fraction, theme: theme, height: 2.5)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .center)
                }
            }
            // A single book must not stretch to the full width — the column is
            // sized by the cover's aspect, and a lone one would draw a cover
            // three times the height of the card.
            if books.count < Layout.mediumColumns {
                ForEach(books.count..<Layout.mediumColumns, id: \.self) { _ in
                    Color.clear.frame(maxWidth: .infinity)
                }
            }
        }
    }
}

// MARK: - Large

/// Five in a list, each with when it was last open — the family with room for
/// the question the other two can't answer.
private struct LargeCard: View {
    let books: [WidgetBook]
    let theme: WidgetTheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Continue")
                .font(.system(size: 10.5, weight: .semibold))
                .tracking(0.7)
                .textCase(.uppercase)
                .foregroundStyle(theme.rule)
                .padding(.bottom, 10)

            ForEach(Array(books.prefix(WidgetSnapshot.maxBooks).enumerated()), id: \.element.id) {
                index, book in
                if index > 0 {
                    Rectangle()
                        .fill(theme.ink2.opacity(0.18))
                        .frame(height: 0.5)
                }
                Link(destination: book.deepLink) { LargeRow(book: book, theme: theme) }
                    // The rows share the card's height rather than stacking at
                    // the top under a spacer. Five is the ceiling but two or
                    // three is the common case, and holding a fixed row height
                    // there left the bottom half of the card visibly empty.
                    .frame(maxHeight: .infinity)
            }
        }
    }
}

private struct LargeRow: View {
    let book: WidgetBook
    let theme: WidgetTheme

    var body: some View {
        HStack(alignment: .center, spacing: 11) {
            WidgetCover(book: book, theme: theme, cornerRadius: 3)
                .frame(height: 48)
                .shadow(color: .black.opacity(0.25), radius: 4, y: 2)

            VStack(alignment: .leading, spacing: 3) {
                Text(book.title)
                    .font(.system(size: 13.5, weight: .semibold, design: .serif))
                    .foregroundStyle(theme.ink0)
                    .lineLimit(1)

                Text(book.author)
                    .font(.system(size: 11))
                    .foregroundStyle(theme.ink1)
                    .lineLimit(1)

                if let fraction = book.fraction {
                    WidgetProgressBar(fraction: fraction, theme: theme, height: 2.5)
                        .padding(.top, 2)
                }
            }

            Spacer(minLength: 4)

            VStack(alignment: .trailing, spacing: 3) {
                Image(systemName: book.format == .audio ? "headphones" : "book.closed.fill")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(theme.rule)

                // Not `Text(_, style: .relative)`. That one re-renders on the
                // system's clock, which is the tempting part, but it formats
                // as a bare duration — a book read two hours ago reads
                // "2 hr, 0 min", which on a card full of audiobooks looks like
                // time *remaining*. This says "2h ago", matching the app's own
                // `Format.relative`, and drifts by at most one refresh.
                Text(WidgetLabels.relative(book.updatedAt))
                    .font(.system(size: 9.5))
                    .foregroundStyle(theme.ink2)
                    .lineLimit(1)
                    .multilineTextAlignment(.trailing)
            }
            .fixedSize(horizontal: true, vertical: false)
        }
    }
}

// MARK: - Shared pieces

/// The bar and the one line under it — percent for a book, time left for an
/// audiobook. The small card's footer; medium draws a bare bar and large has
/// its own row shape.
private struct PositionFooter: View {
    let book: WidgetBook
    let theme: WidgetTheme

    var body: some View {
        VStack(alignment: .center, spacing: 5) {
            if let fraction = book.fraction {
                WidgetProgressBar(fraction: fraction, theme: theme)
            }
            Text(label)
                .font(.system(size: 10.5, weight: .medium))
                .foregroundStyle(theme.ink1)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity)
    }

    /// Percent and time-left together when the book has both — the two answer
    /// different questions ("how far in", "how much longer"), and an audiobook
    /// is the only thing that can say the second.
    private var label: String {
        var parts: [String] = []
        if let fraction = book.fraction {
            parts.append("\(Int((fraction * 100).rounded()))%")
        }
        if let remaining = book.secondsRemaining {
            parts.append("\(WidgetLabels.duration(remaining)) left")
        }
        // Neither: a CFI-only EPUB save has no honest percentage, so the card
        // says what it is rather than showing an empty band.
        if parts.isEmpty {
            parts.append(book.format == .audio ? "In progress" : "Reading")
        }
        return parts.joined(separator: " · ")
    }
}

/// The three ways there is nothing to continue. Kept apart because they want
/// three different next steps — a blank tile tells the reader nothing about
/// which one they're in.
private struct EmptyCard: View {
    let state: WidgetSnapshot.State
    let theme: WidgetTheme
    let family: WidgetFamily

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Spacer(minLength: 0)

            Image(systemName: symbol)
                .font(.system(size: family == .systemSmall ? 20 : 24, weight: .light))
                .foregroundStyle(theme.rule)
                .padding(.bottom, 2)

            Text(headline)
                .font(.system(size: family == .systemSmall ? 14 : 16, weight: .semibold, design: .serif))
                .foregroundStyle(theme.ink0)
                .lineLimit(2)

            Text(detail)
                .font(.system(size: 11))
                .foregroundStyle(theme.ink1)
                .lineLimit(3)

            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .widgetURL(URL(string: "\(DeepLink.scheme)://"))
    }

    private var symbol: String {
        switch state {
        case .signedOut: "person.crop.circle.badge.questionmark"
        case .emptyLibrary: "books.vertical"
        case .nothingInProgress, .ready: "book.closed"
        }
    }

    private var headline: String {
        switch state {
        case .signedOut: "Not signed in"
        case .emptyLibrary: "No books yet"
        case .nothingInProgress, .ready: "Nothing open"
        }
    }

    private var detail: String {
        switch state {
        case .signedOut: "Open Omnibus and connect to your library server."
        case .emptyLibrary: "Point your server at a library, or add a book from the app."
        case .nothingInProgress, .ready: "Start a book and it'll show up here."
        }
    }
}

enum WidgetLabels {
    /// Compact spoken form — "4h 12m". Mirrors `Format.humanDuration` in the
    /// app; the extension can't reach it, and moving the app's whole
    /// formatting enum across the target boundary to share six lines would
    /// drag SwiftUI view code with it.
    static func duration(_ seconds: Double) -> String {
        guard seconds.isFinite, seconds > 0 else { return "0m" }
        let total = Int(seconds.rounded())
        let hours = total / 3600
        let minutes = (total % 3600) / 60
        if hours > 0 { return minutes > 0 ? "\(hours)h \(minutes)m" : "\(hours)h" }
        if minutes > 0 { return "\(minutes)m" }
        return "\(total)s"
    }

    /// "2h ago". Mirrors `Format.relative(unix:)` in the app, so the widget and
    /// the Continue hero describe the same book in the same words.
    static func relative(_ date: Date, relativeTo now: Date = .now) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: now)
    }
}

private extension WidgetBook {
    /// Where a tap on this card lands. The file id rides along so an audiobook
    /// reopens the narration the position was taken in — two narrations of one
    /// book do not share a timeline.
    var deepLink: URL {
        DeepLink.book(uuid: bookUUID, format: format, fileID: fileID).url
    }
}
