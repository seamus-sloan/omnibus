//  ContinueHero.swift
//  Resuming a book is the most frequent thing anyone does in a reading app, so
//  it gets the top of the screen and a real target rather than a card in a rail.
//
//  Multiple books in progress page horizontally in one card's worth of space —
//  a second rail would cost another band of the landing screen for something
//  most people only have two or three of.

import SwiftUI

struct ContinueHero: View {
    let points: [ResumePoint]
    /// Runs after the long-press editor saves, so the card can pick up a title
    /// or cover that it is still drawing from the old metadata.
    var onEdited: () -> Void = {}
    /// Pushes the book's detail screen — reached by tapping the cover or the
    /// title (matching the web hero) or via the long-press menu; the card's
    /// main tap resumes the book.
    var onOpenDetail: (Book) -> Void = { _ in }

    @Environment(\.palette) private var palette
    @State private var selected: ResumePoint.ID?

    private var height: CGFloat { 176 }

    var body: some View {
        VStack(spacing: 10) {
            // A paging `ScrollView` rather than a `TabView(.page)`: that style is
            // a `UIPageViewController`, which brings its own scroll view and
            // fights the landing scroll view it's nested in — a swipe that isn't
            // quite horizontal moves both, and the page it settles on is decided
            // by the gesture the outer view didn't take.
            ScrollView(.horizontal) {
                HStack(spacing: 0) {
                    ForEach(points) { point in
                        HeroCard(point: point, onEdited: onEdited, onOpenDetail: onOpenDetail)
                            // Padding inside the page, so every page is exactly
                            // the carousel's width and the paging stops land on
                            // a card rather than drifting by the inset.
                            .padding(.horizontal, Spacing.screen)
                            .containerRelativeFrame(.horizontal)
                    }
                }
                .scrollTargetLayout()
            }
            .frame(height: height)
            .scrollTargetBehavior(.paging)
            .scrollPosition(id: settledCard)
            .scrollIndicators(.hidden)

            if points.count > 1 {
                dots
            }
        }
        .onAppear { selected = selected ?? points.first?.id }
        .onChange(of: points.map(\.id)) { _, ids in
            // The rail is rewritten on every position save. Holding the card
            // that's showing — rather than resetting to the front — is what
            // stops it sliding back to the first book while you're looking at
            // it. Only a card that has actually left the rail gives up its spot.
            if selected.map(ids.contains) != true {
                selected = ids.first
            }
        }
    }

    /// Drops the nils `scrollPosition` writes mid-gesture. Left as-is they read
    /// as "no card selected", which lit the first dot for a frame on every swipe.
    private var settledCard: Binding<ResumePoint.ID?> {
        Binding(get: { selected }, set: { if let id = $0 { selected = id } })
    }

    /// Position indicator, driven by where the carousel actually settled rather
    /// than by a count of its own, so it can't disagree with the card below it.
    private var dots: some View {
        let index = points.firstIndex { $0.id == selected } ?? 0
        return HStack(spacing: 6) {
            ForEach(points.indices, id: \.self) { position in
                Capsule()
                    .fill(position == index ? palette.ink1Color : palette.ink3Color.opacity(0.4))
                    .frame(width: position == index ? 16 : 5, height: 5)
            }
        }
        .animation(Motion.snap, value: index)
        .accessibilityHidden(true)
    }
}

private struct HeroCard: View {
    let point: ResumePoint
    let onEdited: () -> Void
    let onOpenDetail: (Book) -> Void

    @Environment(\.palette) private var palette

    private var book: Book { point.book }
    private var tone: OKLCH { CoverIdentity(book).tone }

    /// The one place the card decides which medium it's about: the eyebrow, the
    /// bar, the position line, the button, and where the tap goes all read this.
    ///
    /// Normally the format the position was recorded in. A progress row outlives
    /// its file — it soft-references `books.uuid` with no cascade — so a book
    /// whose audiobook has since been removed still carries an audio position,
    /// and offering "Play" for it opens a player with nothing to play. `formats`
    /// is empty only when the payload omitted it, so an empty list defers to the
    /// record rather than overriding it.
    private var format: ProgressFormat {
        guard !book.formats.isEmpty else { return point.record.format }
        switch point.record.format {
        case .audio: return book.hasAudiobook ? .audio : .epub
        case .epub: return book.hasEbook ? .epub : .audio
        }
    }

    private var isAudio: Bool { format == .audio }

    /// The record's own honest fraction — audio's position over its duration,
    /// or a reading record's cross-surface percent (a comic position always
    /// carries one; a CFI-only EPUB save never does) — but not a listening
    /// fraction on a card no longer presenting itself as one.
    private var fraction: Double? {
        if isAudio != point.isAudio { return nil }
        return point.fraction
    }

    private var washTone: OKLCH {
        OKLCH(palette.bg0.l > 0.5 ? 0.93 : 0.24, tone.c * 0.6, tone.h)
    }

    private var rule: Color {
        OKLCH(0.80, tone.c * 0.95, tone.h).color
    }

    var body: some View {
        // The cover and the title each open the detail screen — matching the
        // web hero, where both are links and only the Play/Read pill resumes.
        // The targets are siblings, not buttons nested in a card-wide button:
        // SwiftUI doesn't guarantee tap routing for nested controls, so the
        // card's own resume tap rides the container's gesture instead, which
        // every real button reliably wins.
        HStack(alignment: .top, spacing: 16) {
            Button {
                Haptics.tap()
                onOpenDetail(book)
            } label: {
                BookCover(identity: CoverIdentity(book), size: .md, cornerRadius: 6)
                    .frame(width: 96)
                    .coverShadow(1.2)
            }
            .buttonStyle(BookPressStyle())
            .accessibilityLabel(book.displayTitle)
            .accessibilityHint("Opens book details")

            VStack(alignment: .leading, spacing: 5) {
                Text(isAudio ? "Continue listening" : "Continue reading")
                    .font(.ui(10.5, weight: .semibold))
                    .tracking(0.6)
                    .textCase(.uppercase)
                    .foregroundStyle(rule)

                Button {
                    Haptics.tap()
                    onOpenDetail(book)
                } label: {
                    Text(book.displayTitle)
                        .font(.display(21))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
                .buttonStyle(.plain)
                .accessibilityHint("Opens book details")
                // The card's height is fixed, so a stack with nothing left to
                // give takes it from the title — a two-line one silently became
                // one truncated line the moment anything below asked for space.
                .layoutPriority(1)

                Text(book.authorDisplay)
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink2Color)
                    .lineLimit(1)

                // The stack's own 5pt already separates the author from the
                // bar; this only pushes the bottom block down when there's
                // room, so it must be free to yield all of it when there isn't.
                Spacer(minLength: 0)

                // A CFI-only EPUB resume point has no honest percentage,
                // so there is no bar to draw for one. The band the bar
                // would occupy goes to when you left off instead of sitting
                // empty — which is what made the card read as underfilled.
                if let fraction {
                    // A bare rule carries no leading of its own, so the stack's
                    // spacing alone left it sitting on the Read pill's cap. The
                    // text branch below needs none — its line box is the gap.
                    ProgressBar(fraction: fraction, tint: rule)
                        .padding(.bottom, 5)
                } else {
                    // Sentence case, quietly: the eyebrow above is already
                    // set in caps, and two shouting lines in one card read
                    // as a warning rather than as a detail.
                    Text(lastOpenedLabel)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                }

                HStack(spacing: 8) {
                    Text(positionLabel)
                        .font(.ui(11.5, weight: .medium))
                        .foregroundStyle(palette.ink2Color)

                    Spacer(minLength: 0)

                    Button {
                        resume()
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: isAudio ? "play.fill" : "book.fill")
                                .font(.system(size: 10, weight: .bold))
                            Text(isAudio ? "Play" : "Read")
                                .font(.ui(12, weight: .semibold))
                        }
                        .foregroundStyle(palette.accentInk.color)
                        .padding(.horizontal, 13)
                        .padding(.vertical, 7)
                        .background(Capsule().fill(palette.accentColor))
                    }
                    .buttonStyle(PressableStyle())
                }
            }
        }
        .padding(16)
        // Fills the carousel's fixed height, so a one-line title and a
        // two-line one produce the same card rather than two page sizes.
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(cardGround)
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(
                    LinearGradient(
                        colors: [.white.opacity(0.16), .white.opacity(0.04)],
                        startPoint: .top,
                        endPoint: .bottom
                    ),
                    lineWidth: 0.5
                )
        )
        // Grounds the card against the page instead of letting it float as
        // a flat panel of colour.
        .shadow(color: .black.opacity(0.35), radius: 18, y: 8)
        // The rest of the card keeps the old whole-card behavior: a tap
        // anywhere that isn't the cover, the title, or the pill resumes.
        .contentShape(RoundedRectangle(cornerRadius: Radius.lg, style: .continuous))
        .onTapGesture { resume() }
        // The same held-down menu as a grid cell, plus the way through to the
        // detail screen the card's main tap spends on resuming the book —
        // kept even though the cover and title now go there, so the menu
        // behaves the same on every card.
        .bookContextMenu(book, onEdited: onEdited, onOpenDetail: { onOpenDetail(book) })
    }

    /// The card's main tap: back into the book where you left off.
    private func resume() {
        Haptics.tap()
        if isAudio {
            // Reopen the file the position was taken in, not the book's
            // first one — two narrations don't share a timeline.
            Presentation.shared.openPlayer(book, fileID: point.record.bookFileID)
        } else {
            Presentation.shared.openReader(book)
        }
    }

    /// A lit alcove rather than a filled rectangle: the wash graded top-to-
    /// bottom, with a bloom of the book's own colour behind its cover.
    private var cardGround: some View {
        RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
            .fill(
                LinearGradient(
                    colors: [
                        washTone.color,
                        OKLCH(washTone.l * 0.78, washTone.c * 0.9, tone.h).color,
                    ],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            )
            .overlay(alignment: .leading) {
                // `endRadius` must land inside the frame's half-width, or the
                // frame clips the gradient while it still has colour and the
                // bloom shows a hard vertical seam down the card.
                RadialGradient(
                    colors: [OKLCH(0.62, tone.c * 0.9, tone.h).color.opacity(0.40), .clear],
                    center: .center,
                    startRadius: 0,
                    endRadius: 160
                )
                .frame(width: 320, height: 320)
                .offset(x: -60)
                .blendMode(.plusLighter)
            }
            .clipShape(RoundedRectangle(cornerRadius: Radius.lg, style: .continuous))
    }

    private var positionLabel: String {
        if isAudio {
            if let chapter = point.chapterNumber, let count = point.chapterCount {
                return "Chapter \(chapter) of \(count)"
            }
            if let total = point.totalDurationSeconds,
               let position = point.record.audioPositionSeconds {
                // Rate-adjusted: the wall-clock wait at the saved speed,
                // matching the player's own "left" readout. Clamped first —
                // a saved position past the reported total (stale metadata)
                // must read as 0 left, not a negative span.
                let left = Format.atRate(max(0, total - position), rate: point.playbackRate ?? 1.0)
                return Format.humanDuration(Int64(left)) + " left"
            }
            return "In progress"
        }
        return "Reading"
    }

    private var lastOpenedLabel: String {
        "Last opened \(Format.relative(unix: point.record.updatedAt))"
    }
}
