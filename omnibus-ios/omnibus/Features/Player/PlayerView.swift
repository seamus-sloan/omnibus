//  PlayerView.swift
//  The immersive audiobook player, plus the mini bar docked in the tab bar.

import SwiftUI

struct PlayerView: View {
    let book: Book

    init(book: Book) {
        self.book = book
    }

    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var scrubPosition: Double?
    @State private var showChapters = false
    @State private var showSleepTimer = false
    @State private var showSpeed = false
    @State private var showBookmarks = false

    private static let rates: [Double] = [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0]

    var body: some View {
        ZStack {
            backdrop

            // Artwork + titles own the middle; the transport and the top bar
            // are attached as safe-area insets so they reserve their space
            // instead of competing for it in one VStack — which silently
            // compressed them to nothing on a full-screen cover.
            VStack(spacing: Spacing.lg) {
                Spacer(minLength: 0)
                cover
                titles
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Spacing.screen)

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
                .padding(.vertical, Spacing.sm)
                .containerRelativeFrame(.horizontal)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: Spacing.lg) {
                scrubber
                transport
                secondaryControls
            }
            .padding(.horizontal, Spacing.screen)
            .padding(.top, Spacing.lg)
            .padding(.bottom, Spacing.sm)
            .containerRelativeFrame(.horizontal)
        }
        .background(ScreenBackground())
        .task { await player.load(book: book) }
        .sheet(isPresented: $showChapters) { ChapterSheet() }
        .sheet(isPresented: $showBookmarks) { BookmarksSheet(book: book, isAudio: true) }
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
                RemoteImage(path: "/api/thumbs/\(book.uuid)/md") { Color.clear }
                    .scaledToFill()
                    .blur(radius: 70, opaque: true)
                    .saturation(1.5)
                    .opacity(0.4)
            }

            LinearGradient(
                colors: [palette.bg0Color.opacity(0.2), palette.bg0Color],
                startPoint: .top, endPoint: .bottom
            )
        }
        .ignoresSafeArea()
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

    private var cover: some View {
        BookCover(identity: CoverIdentity(uuid: book.uuid, title: book.displayTitle, hasCover: book.coverURL != nil), size: .lg, cornerRadius: Radius.lg)
        .frame(maxWidth: 320)
        .shadow(color: .black.opacity(0.5), radius: 28, x: 0, y: 16)
        .scaleEffect(player.isPlaying ? 1 : 0.94)
        .animation(Motion.glide, value: player.isPlaying)
    }

    private var titles: some View {
        VStack(spacing: 5) {
            Text(book.displayTitle)
                .font(.display(25))
                .foregroundStyle(palette.ink0Color)
                .multilineTextAlignment(.center)
                .lineLimit(2)
            Text(book.authorDisplay)
                .font(.ui(14))
                .foregroundStyle(palette.ink2Color)
                .lineLimit(1)
            if let chapter = player.currentChapter {
                Button { showChapters = true } label: {
                    Label(chapter.title, systemImage: "list.bullet")
                        .font(.ui(12, weight: .medium))
                        .foregroundStyle(palette.accentColor)
                        .lineLimit(1)
                }
                .buttonStyle(.plain)
                .padding(.top, 2)
            }
        }
    }

    private var scrubber: some View {
        VStack(spacing: 4) {
            Slider(
                value: Binding(
                    get: { min(scrubPosition ?? player.position, sliderUpperBound) },
                    set: { scrubPosition = $0 }
                ),
                in: 0...sliderUpperBound,
                onEditingChanged: { editing in
                    guard !editing, let target = scrubPosition else { return }
                    Task {
                        await player.seek(to: target)
                        scrubPosition = nil
                    }
                }
            )
            .tint(palette.accentColor)

            HStack {
                Text(Format.duration(scrubPosition ?? player.position))
                Spacer()
                Text("-" + Format.duration(max(0, player.duration - (scrubPosition ?? player.position))))
            }
            .font(.monoUI(11))
            .foregroundStyle(palette.ink2Color)
        }
    }

    /// A zero-length range traps `Slider`, and duration is 0 until the asset
    /// resolves, so the upper bound never drops below one second.
    private var sliderUpperBound: Double {
        max(player.duration, 1)
    }

    private var transport: some View {
        HStack(spacing: 0) {
            transportButton("backward.end.fill", size: 20) { player.previousChapter() }
            transportButton("gobackward.15", size: 29) { player.skip(-15) }

            Button { player.toggle() } label: {
                // Solid rather than hierarchical: hierarchical renders the disc
                // at a fraction of the ink, which left the one control the
                // thumb reaches for as the dimmest thing in the transport.
                Image(systemName: player.isPlaying ? "pause.circle.fill" : "play.circle.fill")
                    .font(.system(size: 66))
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(palette.bg0Color, palette.ink0Color)
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity)

            transportButton("goforward.30", size: 29) { player.skip(30) }
            transportButton("forward.end.fill", size: 20) { player.nextChapter() }
        }
    }

    private func transportButton(
        _ icon: String, size: CGFloat, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: size))
                .foregroundStyle(palette.ink0Color)
                .frame(maxWidth: .infinity)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// Four evenly-weighted slots. Every entry is a plain `Button` driving a
    /// confirmation dialog — a `Menu` here silently dropped out of the row.
    private var secondaryControls: some View {
        HStack(spacing: 0) {
            controlChip(Self.rateLabel(player.rate), icon: "speedometer") { showSpeed = true }
            controlChip("Chapters", icon: "list.bullet") { showChapters = true }
                .disabled(player.chapters.isEmpty)
            controlChip("Bookmark", icon: "bookmark") {
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
            controlChip("Sleep", icon: "moon.zzz") { showSleepTimer = true }
        }
    }

    private func controlChip(
        _ label: String, icon: String, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 16))
                Text(label).font(.ui(10)).lineLimit(1)
            }
            .foregroundStyle(palette.ink2Color)
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
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
                List(player.chapters) { chapter in
                    Button {
                        player.seekToChapter(chapter)
                        dismiss()
                    } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(chapter.title)
                                    .font(.ui(15, weight: .medium))
                                    .foregroundStyle(palette.ink0Color)
                                Text(Format.duration(chapter.startSeconds))
                                    .font(.monoUI(11))
                                    .foregroundStyle(palette.ink3Color)
                            }
                            Spacer()
                            if player.currentChapter?.ordinal == chapter.ordinal {
                                Image(systemName: "waveform")
                                    .foregroundStyle(palette.accentColor)
                                    .symbolEffect(.variableColor, isActive: player.isPlaying)
                            }
                        }
                    }
                    .listRowBackground(palette.bg1Color)
                    .id(chapter.ordinal)
                }
                .scrollContentBackground(.hidden)
                .background(palette.bg0Color)
                .onAppear {
                    guard let current = player.currentChapter else { return }
                    proxy.scrollTo(current.ordinal, anchor: .center)
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
        .presentationDetents([.medium, .large])
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

    var body: some View {
        if let book = player.book {
            VStack(spacing: 0) {
                // A hairline of progress along the top edge — the only place
                // position is visible without opening the player.
                ProgressBar(
                    fraction: player.duration > 0 ? player.position / player.duration : 0,
                    tint: palette.accentColor,
                    height: 2
                )

                Button {
                    Presentation.shared.openPlayer(book)
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
