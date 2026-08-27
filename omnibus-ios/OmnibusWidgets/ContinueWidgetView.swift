//  ContinueWidgetView.swift
//  One card per family, plus the three ways the card can be empty.
//
//  The families are not the same layout at three sizes: small and medium each
//  answer "what am I in the middle of" with a single book, and large answers
//  "and when did I last touch each" with all of them. The one-book families
//  carry a control that flips to the next book, because a widget cannot be
//  swiped — see `ContinueIntents`.

import AppIntents
import SwiftUI
import WidgetKit

struct ContinueWidgetView: View {
    let snapshot: WidgetSnapshot
    /// Which book the one-book families are showing, by `WidgetBook.id`.
    var cursor: String?

    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var scheme

    /// The whole card takes its colour from the book it is about, the way the
    /// book-detail hero takes its wash from the book it is about. For the one
    /// book families that is whichever the reader has flipped to; large is a
    /// list, so it stays with the book leading it.
    private var theme: WidgetTheme {
        WidgetTheme(tone: (isRail ? snapshot.books.first : shown?.book)?.tone, scheme: scheme)
    }

    private var isRail: Bool { family == .systemLarge }

    private var shown: (book: WidgetBook, index: Int)? {
        snapshot.showing(cursor: cursor)
    }

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .containerBackground(for: .widget) { background }
    }

    /// Small spends the book's own artwork on its ground; the others keep the
    /// tone wash, which is what a card with room for the cover *sharp* wants
    /// behind it.
    @ViewBuilder
    private var background: some View {
        if family == .systemSmall, let book = shown?.book {
            WidgetHeroBackdrop(book: book, theme: theme)
        } else {
            theme.ground
        }
    }

    @ViewBuilder
    private var content: some View {
        if let shown, !snapshot.books.isEmpty {
            let rail = RailPosition(index: shown.index, count: snapshot.books.count)
            switch family {
            case .systemSmall: SmallCard(book: shown.book, theme: theme, rail: rail)
            case .systemLarge: LargeCard(books: snapshot.books, theme: theme)
            default: MediumCard(book: shown.book, theme: theme, rail: rail)
            }
        } else {
            EmptyCard(state: snapshot.state, theme: theme, family: family)
        }
    }
}

private enum Layout {
    static let margin: CGFloat = 14
}

/// Where the shown book sits in the rail — what the dots draw and what tells
/// the advance control whether there is anywhere to go.
private struct RailPosition {
    let index: Int
    let count: Int

    var hasOthers: Bool { count > 1 }
}

// MARK: - Small

/// One book, given the whole card: its own artwork blurred into the ground, the
/// cover sharp on top, and where you are along the bottom edge.
///
/// The sizes here are a budget, not a preference. A `systemSmall` is 130pt of
/// content on a 6.3" phone, and the cover, the spacings and the position line
/// are all fixed — so whatever they don't claim is what the title gets, and
/// `layoutPriority` cannot conjure more. Sized so the title keeps two lines:
/// nothing on this card matters more than which book it is.
private struct SmallCard: View {
    let book: WidgetBook
    let theme: WidgetTheme
    let rail: RailPosition

    var body: some View {
        // Centred as a column. A 2:3 cover is far narrower than the card, so
        // ranged left it sat off in one corner — and centring the cover over a
        // left-ranged title just moves the mismatch onto the type.
        VStack(alignment: .center, spacing: 0) {
            WidgetCover(book: book, theme: theme, cornerRadius: 6)
                .frame(height: 74)
                // Deeper than the flat card's, because here the cover is laid
                // on a blur of itself: without a shadow to separate them the
                // sharp edge reads as an artefact of the blur rather than as a
                // second object in front of it.
                .shadow(color: .black.opacity(0.42), radius: 10, y: 5)
                .frame(maxWidth: .infinity)

            Spacer(minLength: 4)

            Text(book.title)
                .font(.system(size: 13, weight: .semibold, design: .serif))
                .foregroundStyle(theme.ink0)
                .lineLimit(2)
                .multilineTextAlignment(.center)
                // Claims its two lines ahead of the spacer above it.
                .layoutPriority(1)

            Text(PositionLabel.text(for: book))
                .font(.system(size: 9.5, weight: .medium))
                .foregroundStyle(theme.ink1)
                .lineLimit(1)
                .padding(.top, 2)
        }
        .padding(Layout.margin)
        // The bar bleeds the full width of the card rather than sitting inside
        // the margins: it is the one element that reads at arm's length, and
        // the card has no vertical room left to give it a band of its own.
        .overlay(alignment: .bottom) { BleedBar(book: book, theme: theme) }
        .overlay(alignment: .topTrailing) {
            AdvanceButton(theme: theme, rail: rail).padding(6)
        }
        .widgetURL(book.deepLink)
    }
}

/// The position bar, run edge to edge along the bottom of the small card.
private struct BleedBar: View {
    let book: WidgetBook
    let theme: WidgetTheme

    var body: some View {
        if let fraction = book.fraction {
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Rectangle().fill(theme.track)
                    Rectangle()
                        .fill(theme.rule)
                        .frame(width: geometry.size.width * min(1, max(0, fraction)))
                }
            }
            .frame(height: 3)
        }
    }
}

// MARK: - Medium

/// One book as a call to action: the cover at full card height, the title set
/// large beside it, and a pill that resumes it.
///
/// The old three-across fan answered "which of my two or three" by shrinking
/// all three to a thumbnail and a truncated title. This answers it by showing
/// one book properly and letting the reader flip — which is the same question
/// with the resume actually reachable from the Home Screen.
private struct MediumCard: View {
    let book: WidgetBook
    let theme: WidgetTheme
    let rail: RailPosition

    private var isAudio: Bool { book.format == .audio }

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            OptionalLink(destination: book.deepLink) {
                // Sized by width, not height. `WidgetCover` fits a 2:3 box into
                // whatever it is proposed, and an `HStack` proposes width to a
                // flexible child before it knows the row's height — so left to
                // fill, the cover took half the card across and hung off the
                // bottom of it. 86pt puts its derived height just inside the
                // 6.3" card's 130.
                WidgetCover(book: book, theme: theme, cornerRadius: 7)
                    .frame(width: 86)
                    // Tighter than the small card's, which is laid on a blur of
                    // itself and needs the separation. Here the ground is a
                    // pale wash, and a soft shadow on one spreads 10pt of grey
                    // past every edge — which reads as the cover being bigger
                    // and hanging off the card rather than as depth.
                    .shadow(color: .black.opacity(0.26), radius: 6, y: 3)
            }
            .frame(maxHeight: .infinity)

            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .center, spacing: 6) {
                    Text(isAudio ? "Continue listening" : "Continue reading")
                        .font(.system(size: 9.5, weight: .semibold))
                        .tracking(0.6)
                        .textCase(.uppercase)
                        .foregroundStyle(theme.rule)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)

                    Spacer(minLength: 0)

                    RailDots(theme: theme, rail: rail)
                    AdvanceButton(theme: theme, rail: rail)
                }

                Text(book.title)
                    .font(.system(size: 17, weight: .semibold, design: .serif))
                    .foregroundStyle(theme.ink0)
                    .lineLimit(2)
                    // The card's height is fixed, so a stack with nothing left
                    // to give takes it from the title — a two-line one silently
                    // becomes one truncated line the moment anything below asks
                    // for space.
                    .layoutPriority(1)

                Text(book.author)
                    .font(.system(size: 11))
                    .foregroundStyle(theme.ink2)
                    .lineLimit(1)

                Spacer(minLength: 2)

                if let fraction = book.fraction {
                    WidgetProgressBar(fraction: fraction, theme: theme, height: 3)
                        .padding(.bottom, 4)
                }

                HStack(spacing: 8) {
                    Text(PositionLabel.text(for: book))
                        .font(.system(size: 10.5, weight: .medium))
                        .foregroundStyle(theme.ink1)
                        .lineLimit(1)

                    Spacer(minLength: 0)

                    ResumePill(book: book, theme: theme)
                }
            }
        }
        .padding(Layout.margin)
        .background(alignment: .leading) {
            // Centred on the cover: the offset puts the bloom's middle at the
            // margin plus half the cover's width. Kept close to the card's own
            // height, too — a circle much larger than the card is clipped top
            // and bottom into a full-height band, which reads as a slab of
            // colour behind the artwork rather than as a glow around it.
            theme.bloom(diameter: 170).offset(x: Layout.margin + 43 - 85)
        }
        // Everything a `Link` or a `Button` doesn't cover — the margins, the
        // gap between the cover and the text, the text itself. Without it a tap
        // there launches the app with no URL at all, which is the failure
        // `DeepLink` exists to describe.
        .widgetURL(book.deepLink)
    }
}

/// The card's one solid shape, and the only control on it that resumes.
private struct ResumePill: View {
    let book: WidgetBook
    let theme: WidgetTheme

    var body: some View {
        OptionalLink(destination: book.deepLink) {
            HStack(spacing: 5) {
                Image(systemName: book.format == .audio ? "play.fill" : "book.fill")
                    .font(.system(size: 9, weight: .bold))
                Text(book.format == .audio ? "Play" : "Read")
                    .font(.system(size: 11.5, weight: .semibold))
            }
            .foregroundStyle(theme.pillInk)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(Capsule().fill(theme.pillFill))
        }
    }
}

// MARK: - Flipping through the rail

/// Moves the card on to the next book without leaving the Home Screen.
///
/// Hidden on a rail of one, where it would be a control that does nothing —
/// and drawn as a chevron rather than a pair of arrows because the intent
/// wraps, so forward is the only direction there needs to be.
private struct AdvanceButton: View {
    let theme: WidgetTheme
    let rail: RailPosition

    var body: some View {
        if rail.hasOthers {
            Button(intent: ShowNextBook()) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(theme.ink1)
                    .frame(width: 22, height: 22)
                    .background(Circle().fill(theme.track))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Next book")
        }
    }
}

/// Where the shown book sits in the rail. Drawn from the resolved index rather
/// than from a count of its own, so it cannot disagree with the card above it.
private struct RailDots: View {
    let theme: WidgetTheme
    let rail: RailPosition

    var body: some View {
        if rail.hasOthers {
            HStack(spacing: 4) {
                ForEach(0..<rail.count, id: \.self) { position in
                    Capsule()
                        .fill(position == rail.index ? theme.rule : theme.ink2.opacity(0.35))
                        .frame(width: position == rail.index ? 10 : 4, height: 4)
                }
            }
            .accessibilityHidden(true)
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
                OptionalLink(destination: book.deepLink) {
                    LargeRow(book: book, theme: theme)
                }
                    // The rows share the card's height rather than stacking at
                    // the top under a spacer. Five is the ceiling but two or
                    // three is the common case, and holding a fixed row height
                    // there left the bottom half of the card visibly empty.
                    .frame(maxHeight: .infinity)
            }
        }
        .padding(Layout.margin)
        // The kicker, the container margins and the row hairlines are all
        // dead zones otherwise — see `MediumCard`.
        .widgetURL(books.first?.deepLink)
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
                    // The only thing on the row distinguishing an audiobook
                    // from an ebook, and VoiceOver reads a bare system image
                    // as its symbol name — "headphones" is not user-facing.
                    .accessibilityLabel(book.format == .audio ? "Audiobook" : "Ebook")

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

/// The one line describing where you are — percent for a book, time left for an
/// audiobook. Large has its own row shape and doesn't use it.
private enum PositionLabel {
    /// Percent and time-left together when the book has both — the two answer
    /// different questions ("how far in", "how much longer"), and an audiobook
    /// is the only thing that can say the second.
    static func text(for book: WidgetBook) -> String {
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
        .padding(Layout.margin)
        .frame(maxWidth: .infinity, alignment: .leading)
        .widgetURL(DeepLink.appRoot)
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

/// A `Link` when there is somewhere to go, and inert content when there isn't.
///
/// `DeepLink.url` is optional because it refuses to force-unwrap its own
/// fallback; `Link` needs a `URL`. Rather than each call site growing an `if
/// let` that changes the view type, this keeps the layout identical either way.
private struct OptionalLink<Content: View>: View {
    let destination: URL?
    @ViewBuilder var content: () -> Content

    var body: some View {
        if let destination {
            Link(destination: destination) { content() }
        } else {
            content()
        }
    }
}

private extension WidgetBook {
    /// Where a tap on this card lands. The file id rides along so an audiobook
    /// reopens the narration the position was taken in — two narrations of one
    /// book do not share a timeline.
    var deepLink: URL? {
        DeepLink.book(uuid: bookUUID, format: format, fileID: fileID).url
    }
}
