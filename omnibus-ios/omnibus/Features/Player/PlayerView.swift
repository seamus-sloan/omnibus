//  PlayerView.swift
//  The immersive audiobook player, plus the mini bar docked in the tab bar.

import SwiftUI

struct PlayerView: View {
    let book: Book
    let fileID: Int64?

    init(book: Book, fileID: Int64? = nil) {
        self.book = book
        self.fileID = fileID
    }

    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    /// Offset inside the scrubbed span while a drag is in flight.
    @State private var scrubOffset: Double?
    @State private var showChapters = false
    @State private var showSleepTimer = false
    @State private var showSpeed = false
    @State private var showBookmarks = false
    @State private var showCarMode = false

    private static let rates: [Double] = [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0]

    /// Share of the band between the two insets the artwork may take.
    ///
    /// It tapers on a short screen. The transport's height is fixed, so on a
    /// 4.7" phone it eats a much larger fraction of the screen than on a 6.3"
    /// one, and holding the artwork at the tall-screen share there leaves the
    /// title with nothing — which is the squeeze this whole layout exists to
    /// avoid.
    private static func coverShare(in bandHeight: CGFloat) -> CGFloat {
        bandHeight < 380 ? 0.66 : 0.76
    }

    var body: some View {
        ZStack {
            backdrop
            stage

            if player.isLoading {
                LoadingView(label: "Preparing audio")
                    .background(palette.bg0Color.opacity(0.6))
            }

            if let offer = player.syncOffer {
                SyncOfferBanner(
                    title: "Listened further elsewhere",
                    detail: "Another device left off at \(Format.duration(offer)).",
                    onGo: { Task { await player.acceptSyncOffer() } },
                    onDismiss: { player.dismissSyncOffer() }
                )
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        // `containerRelativeFrame` pins these rows to a definite width. Without
        // it they inherit an unbounded proposal from the cover stack, and every
        // `Spacer` / `maxWidth: .infinity` child expands past the screen edge —
        // which is what silently pushed the transport ends and the top bar out
        // of view.
        .safeAreaInset(edge: .top, spacing: 0) {
            topBar
                .padding(.horizontal, Spacing.screen)
                .padding(.vertical, Spacing.xs)
                .containerRelativeFrame(.horizontal)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            controls
                .padding(.horizontal, Spacing.screen)
                .padding(.top, Spacing.sm)
                .padding(.bottom, Spacing.xs)
                .containerRelativeFrame(.horizontal)
        }
        .background(ScreenBackground())
        .task { await player.load(book: book, fileID: fileID) }
        .sheet(isPresented: $showChapters) { ChapterSheet() }
        .sheet(isPresented: $showBookmarks) { BookmarksSheet(book: book, isAudio: true) }
        .fullScreenCover(isPresented: $showCarMode) { CarModeView(book: book) }
        .confirmationDialog("Playback speed", isPresented: $showSpeed, titleVisibility: .visible) {
            ForEach(Self.rates, id: \.self) { value in
                Button(Self.rateLabel(value)) { player.rate = value }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog("Sleep timer", isPresented: $showSleepTimer, titleVisibility: .visible) {
            ForEach([5, 10, 15, 30, 45, 60], id: \.self) { minutes in
                Button("\(minutes) minutes") { player.startSleepTimer(minutes: minutes) }
            }
            if player.sleepMinutesRemaining != nil {
                Button("Turn off", role: .destructive) { player.cancelSleepTimer() }
            }
            Button("Cancel", role: .cancel) {}
        }
    }

    /// A blurred, saturated wash of the cover behind the controls.
    ///
    /// The tone bloom sits under the artwork rather than instead of it: a book
    /// with no cover art used to get a flat black player while the book itself
    /// was drawn in its own colour everywhere else in the app, and a cover that
    /// is mostly dark washed out to the same black.
    private var backdrop: some View {
        let tone = CoverIdentity(book).tone

        return ZStack {
            palette.bg0Color

            RadialGradient(
                colors: [
                    OKLCH(0.45, max(0.06, tone.c) * 1.05, tone.h).color,
                    OKLCH(0.24, tone.c * 0.7, tone.h).color.opacity(0.5),
                    .clear,
                ],
                center: UnitPoint(x: 0.5, y: 0.32),
                startRadius: 0,
                endRadius: 460
            )

            if book.coverURL != nil {
                // Held inside a flexible `Color` rather than sat in the stack
                // directly. An aspect-*fill* image reports a size bigger than
                // the box it fills — that is what filling means — and as a bare
                // `ZStack` sibling that grew the stack past the screen, at which
                // point `safeAreaInset` hung the top bar under the status bar and
                // pushed the utility row's labels off the bottom edge. An
                // overlay can't grow its parent, which is the same reason
                // `BookCover` puts its art in one.
                //
                // Only books with real artwork could hit it; a generated plate
                // is gradients all the way down, and those are flexible.
                Color.clear
                    .overlay {
                        RemoteImage(path: "/api/thumbs/\(book.uuid)/md") { Color.clear }
                            .blur(radius: 70, opaque: true)
                            .saturation(1.5)
                            .opacity(0.4)
                    }
                    .clipped()
            }

            LinearGradient(
                colors: [palette.bg0Color.opacity(0.2), palette.bg0Color],
                startPoint: .top, endPoint: .bottom
            )
        }
        .ignoresSafeArea()
    }

    /// Artwork and titles, in the band the two insets leave.
    ///
    /// The cover takes a definite share of that band rather than *being* what's
    /// left of it. As the remainder it resized between books — a title that
    /// wrapped to two lines cost it 30pt of height — and on a short phone it
    /// drove both spacers to zero, so the artwork sat wedged against the
    /// scrubber with no margin anywhere on the screen.
    private var stage: some View {
        GeometryReader { band in
            VStack(spacing: Spacing.md) {
                Spacer(minLength: 0)
                cover(maxHeight: max(140, band.size.height * Self.coverShare(in: band.size.height)))
                titles
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(.horizontal, Spacing.screen)
    }

    private var topBar: some View {
        HStack(spacing: Spacing.md) {
            circleButton("chevron.down") { dismiss() }

            Spacer()

            if let minutes = player.sleepMinutesRemaining {
                Label("\(minutes)m", systemImage: "moon.zzz.fill")
                    .font(.ui(12, weight: .medium))
                    .foregroundStyle(palette.accentColor)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(Capsule().fill(palette.bg2Color))
            }

            if book.hasEbook {
                circleButton("book") { Presentation.shared.openReader(book) }
            }
            circleButton("bookmark") { showBookmarks = true }
        }
        .frame(height: 40)
    }

    private func circleButton(_ icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(palette.ink1Color)
                .frame(width: 38, height: 38)
                .background(Circle().fill(palette.bg2Color))
        }
        .buttonStyle(.plain)
    }

    /// Takes the whole `CoverIdentity`, accent included, so the generated plate
    /// can't disagree with the wash `backdrop` derives from the same book.
    private func cover(maxHeight: CGFloat) -> some View {
        BookCover(identity: CoverIdentity(book), size: .lg, cornerRadius: Radius.lg)
            .frame(maxWidth: 300, maxHeight: maxHeight)
            .shadow(color: .black.opacity(0.5), radius: 28, x: 0, y: 16)
            .scaleEffect(player.isPlaying ? 1 : 0.94)
            .animation(Motion.glide, value: player.isPlaying)
    }

    private var titles: some View {
        VStack(spacing: 4) {
            Text(book.displayTitle)
                .font(.display(25))
                .foregroundStyle(palette.ink0Color)
                .multilineTextAlignment(.center)
                .lineLimit(2)
            Text(book.authorDisplay)
                .font(.ui(14))
                .foregroundStyle(palette.ink2Color)
                .lineLimit(1)
        }
    }

    // MARK: - Controls

    private var controls: some View {
        VStack(spacing: 0) {
            chapterRow
            scrubber
                .padding(.top, Spacing.sm)
            transport
                .padding(.top, Spacing.sm)
            utilityRow
                .padding(.top, Spacing.md)
        }
    }

    /// The chapter name, above the scrubber and tappable straight into the
    /// chapter list. It carries the most weight of anything down here because
    /// "where am I" is a chapter name before it's a timestamp.
    @ViewBuilder
    private var chapterRow: some View {
        if let chapter = player.currentChapter {
            Button { showChapters = true } label: {
                HStack(spacing: Spacing.sm) {
                    Image(systemName: "list.bullet")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(palette.ink2Color)
                    Text(chapter.title)
                        .font(.ui(17, weight: .semibold))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .frame(height: 38)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    /// Scoped to the current chapter: one bar width is one chapter, which is the
    /// span you can actually aim inside — on a twelve-hour book a whole-book bar
    /// puts every chapter boundary within a couple of points of its neighbours.
    ///
    /// A book with no chapter marks falls back to the whole book, which is what
    /// `chapterStart` / `chapterDuration` report in that case.
    private var scrubber: some View {
        VStack(spacing: Spacing.xs) {
            PlayerScrubber(
                fraction: displayedOffset / spanUpperBound,
                tint: palette.accentColor,
                onScrub: { scrubOffset = $0 * spanUpperBound },
                onCommit: { target in
                    let seconds = target * spanUpperBound
                    scrubOffset = seconds
                    Task {
                        await player.seekWithinChapter(to: seconds)
                        scrubOffset = nil
                    }
                }
            )

            // Chapter elapsed, book remaining, chapter remaining — all three at
            // once. The middle one is the whole-book context that a
            // chapter-scoped bar would otherwise cost you.
            HStack(spacing: Spacing.sm) {
                Text(Format.duration(displayedOffset))
                    .frame(maxWidth: .infinity, alignment: .leading)

                if player.hasChapters {
                    Text(Format.humanDuration(Int64(bookRemaining)) + " left")
                }

                Text(chapterRemainingLabel)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .font(.ui(12))
            .monospacedDigit()
            .foregroundStyle(palette.ink2Color)
            .frame(height: 16)
        }
    }

    /// What the thumb and the left readout should show: the drag in flight when
    /// there is one, otherwise where playback actually is.
    private var displayedOffset: Double {
        min(scrubOffset ?? player.chapterOffset, spanUpperBound)
    }

    /// The span is 0 until the asset resolves, and it divides the scrubber's
    /// fraction, so the upper bound never drops below one second.
    private var spanUpperBound: Double {
        max(player.chapterDuration, 1)
    }

    private var bookRemaining: Double {
        max(0, player.duration - (player.chapterStart + displayedOffset))
    }

    private var chapterRemainingLabel: String {
        let left = max(0, player.chapterDuration - displayedOffset)
        // No sign at the end of a chapter: "-0:00" reads as a broken clock.
        return left < 1 ? "0:00" : "- " + Format.duration(left)
    }

    /// Five slots: chapter back, skip back, play/pause, skip forward, chapter
    /// forward. The play disc is intrinsically sized rather than taking an equal
    /// fifth, so the four steppers split what's left and it can't be squeezed.
    private var transport: some View {
        HStack(spacing: 0) {
            transportButton("backward.end.fill", size: 26, enabled: player.canGoPreviousChapter) {
                player.previousChapter()
            }
            transportButton("gobackward.15", size: 32) { player.skip(-15) }

            Button { player.toggle() } label: {
                // Solid rather than hierarchical: hierarchical renders the disc
                // at a fraction of the ink, which left the one control the
                // thumb reaches for as the dimmest thing in the transport.
                Image(systemName: player.isPlaying ? "pause.circle.fill" : "play.circle.fill")
                    .font(.system(size: 74))
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(palette.bg0Color, palette.ink0Color)
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.plain)

            transportButton("goforward.30", size: 32) { player.skip(30) }
            transportButton("forward.end.fill", size: 26, enabled: player.canGoNextChapter) {
                player.nextChapter()
            }
        }
        .frame(height: 78)
    }

    private func transportButton(
        _ icon: String, size: CGFloat, enabled: Bool = true, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: size))
                .foregroundStyle(enabled ? palette.ink0Color : palette.ink3Color)
                .frame(maxWidth: .infinity)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    /// Speed · Car Mode · Sleep · Bookmark. Every entry is a plain `Button`
    /// driving a confirmation dialog or a cover — a `Menu` here silently dropped
    /// out of the row.
    private var utilityRow: some View {
        HStack(spacing: 0) {
            utilityCell("Speed") { showSpeed = true } glyph: {
                // The rate reads as the icon, so the current speed is legible
                // without opening anything.
                Text(Self.rateLabel(player.rate)).font(.ui(19, weight: .semibold))
            }
            utilityCell("Car Mode") { showCarMode = true } glyph: {
                Image(systemName: "car").font(.system(size: 22))
            }
            utilityCell("Sleep") { showSleepTimer = true } glyph: {
                Image(systemName: "moon.zzz").font(.system(size: 22))
            }
            utilityCell("Bookmark") { addBookmark() } glyph: {
                Image(systemName: "bookmark").font(.system(size: 22))
            }
        }
    }

    private func utilityCell<Glyph: View>(
        _ label: String,
        action: @escaping () -> Void,
        @ViewBuilder glyph: () -> Glyph
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 5) {
                glyph()
                    .frame(height: 26)
                Text(label)
                    .font(.ui(11))
                    .lineLimit(1)
            }
            .foregroundStyle(palette.ink2Color)
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func addBookmark() {
        Task {
            await UserDataService.createBookmark(
                CreateBookmark(
                    bookUUID: book.uuid,
                    position: String(player.position),
                    title: player.currentChapter?.title
                )
            )
            Haptics.success()
        }
    }

    private static func rateLabel(_ value: Double) -> String {
        value == value.rounded()
            ? String(format: "%.0fx", value)
            : String(format: "%gx", value)
    }
}


// MARK: - Chapter sheet

private struct ChapterSheet: View {
    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                // Keyed on position rather than `ordinal`: the ordinal comes out
                // of the audio container and isn't guaranteed unique, which a
                // `List` id has to be.
                List(Array(player.chapters.enumerated()), id: \.offset) { index, chapter in
                    row(index: index, chapter: chapter)
                        .listRowBackground(palette.bg1Color)
                        .id(index)
                }
                .scrollContentBackground(.hidden)
                .background(palette.bg0Color)
                .onAppear {
                    guard let current = player.currentChapterIndex else { return }
                    proxy.scrollTo(current, anchor: .center)
                }
            }
            .navigationTitle("Chapters")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        // Full height, matching the reader's contents sheet — same job, and a
        // half-height sheet shows four rows of a book that can have a hundred
        // chapters, which is not enough to navigate by.
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
    }

    private func row(index: Int, chapter: ChapterInfo) -> some View {
        let isCurrent = player.currentChapterIndex == index

        return Button {
            player.seekToChapter(chapter)
            dismiss()
        } label: {
            HStack(spacing: Spacing.md) {
                Text("\(index + 1)")
                    .font(.monoUI(12))
                    .foregroundStyle(isCurrent ? palette.accentColor : palette.ink3Color)
                    .frame(width: 24, alignment: .trailing)

                VStack(alignment: .leading, spacing: 5) {
                    Text(chapter.title)
                        .font(.ui(15, weight: isCurrent ? .semibold : .regular))
                        .foregroundStyle(isCurrent ? palette.ink0Color : palette.ink1Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)

                    // Only the chapter you're in gets a bar. On the others the
                    // number that answers "should I start this now?" is how
                    // long it runs, which is the column on the right.
                    if isCurrent {
                        ProgressBar(fraction: fraction, tint: palette.accentColor, height: 2)
                            .frame(height: 2)
                    }
                }

                Spacer(minLength: Spacing.sm)

                Text(Format.duration(player.chapterLength(at: index)))
                    .font(.monoUI(11))
                    .foregroundStyle(palette.ink3Color)
            }
            .padding(.vertical, 3)
            // Without this only the drawn glyphs take the tap, so the gap
            // between a short chapter title and its duration was dead — the row
            // looked tappable across its width and wasn't.
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var fraction: Double {
        guard player.chapterDuration > 0 else { return 0 }
        return min(1, max(0, player.chapterOffset / player.chapterDuration))
    }
}

// MARK: - Mini player

/// Sits directly above the tab bar. It supplies its own material and rule —
/// as a plain view in the bar stack it no longer inherits the chrome the
/// system accessory slot used to provide, and without them the page scrolls
/// visibly through it.
struct MiniPlayerBar: View {
    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette

    /// Overrides what tapping the bar body does. The default expands into the
    /// app-global full-screen player; the reader passes its own handler so the
    /// book underneath stays open.
    var onExpand: (() -> Void)? = nil

    var body: some View {
        if let book = player.book {
            VStack(spacing: 0) {
                // A hairline of progress along the top edge — the only place
                // position is visible without opening the player. Whole-book,
                // deliberately: the player's own bar is chapter-scoped, so this
                // is where the other half of the picture lives.
                ProgressBar(
                    fraction: player.duration > 0 ? player.position / player.duration : 0,
                    tint: palette.accentColor,
                    height: 2
                )

                Button {
                    // The file already playing, so expanding cannot reload it.
                    if let onExpand {
                        onExpand()
                    } else {
                        Presentation.shared.openPlayer(book, fileID: player.fileID)
                    }
                } label: {
                    HStack(spacing: 11) {
                        BookCover(identity: CoverIdentity(book), size: .sm, cornerRadius: 4)
                            .frame(width: 34)

                        VStack(alignment: .leading, spacing: 1) {
                            Text(book.displayTitle)
                                .font(.ui(13.5, weight: .semibold))
                                .foregroundStyle(palette.ink0Color)
                                .lineLimit(1)
                            Text(player.currentChapter?.title ?? book.authorDisplay)
                                .font(.ui(11))
                                .foregroundStyle(palette.ink3Color)
                                .lineLimit(1)
                        }

                        Spacer(minLength: 0)

                        control("gobackward.15") { player.skip(-15) }
                        control(player.isPlaying ? "pause.fill" : "play.fill") { player.toggle() }
                        // The bar had no way out, so `close()` — which is what
                        // flushes the position and reports the listening
                        // session — was unreachable and never ran.
                        control("xmark") { player.close() }
                            .accessibilityLabel("Stop playback")
                    }
                    .padding(.horizontal, Spacing.screen)
                    .padding(.vertical, 9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .background(palette.bg1Color.opacity(0.92))
            .background(.ultraThinMaterial)
            .overlay(alignment: .top) {
                Rectangle().fill(palette.line2.color).frame(height: 0.5)
            }
        }
    }

    private func control(_ icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 17))
                .foregroundStyle(palette.ink0Color)
                .frame(width: 36, height: 36)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
