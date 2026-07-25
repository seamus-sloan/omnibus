//  ReaderView.swift
//  Native chrome around the epub.js stage: tap zones for page turns, an
//  auto-hiding top/bottom bar, the typography sheet, TOC, and the selection
//  action bar that creates highlights.

import SwiftUI

/// A position worth being able to get back to after a scrub.
struct ReturnPoint: Equatable {
    let cfi: String
    let page: Int
}

struct ReaderView: View {
    let book: Book

    init(book: Book) {
        self.book = book
    }

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) private var appState

    @State private var controller = ReaderController()
    @State private var chromeVisible = true
    @State private var showSettings = false
    @State private var showContents = false
    @State private var contentsTab: ReaderContentsSheet.Tab = .contents
    @State private var showMenu = false
    @State private var highlights: [Highlight] = []
    /// Only the count is needed here — the list itself is the sheet's business.
    @State private var bookmarkCount = 0
    /// Where a scrub started, so there's a way back from a mis-drag.
    @State private var returnPoint: ReturnPoint?
    /// Live position while the Contents row is being scrubbed, driving the
    /// chapter/page readout above it.
    @State private var scrubFraction: Double?
    /// Flips the ribbon solid for a beat after saving a bookmark — the only
    /// confirmation that an instant, invisible action actually happened.
    @State private var justBookmarked = false
    /// The highlight whose note is being written, driving the composer sheet.
    @State private var noteTarget: Highlight?
    @State private var startCFI: String?
    @State private var didConfigure = false
    @State private var sessionStart = Date()
    @State private var lastSave = Date.distantPast

    private var bridge = ReaderBridge.shared

    var body: some View {
        ZStack {
            readerPage.ignoresSafeArea()

            if didConfigure {
                ReaderWebView(controller: controller, bookUUID: book.uuid)
                    .ignoresSafeArea()
                    .opacity(controller.isReady ? 1 : 0)
                    // Fade the first paint in rather than letting a
                    // half-laid-out page flash — the reader is the one surface
                    // where a transient wrong state is most visible.
                    .animation(Motion.page, value: controller.isReady)
            }

            if !controller.isReady && !controller.failed {
                LoadingView(label: "Opening \(book.displayTitle)")
            }

            if controller.failed {
                ErrorStateView(message: "This book couldn't be opened.") { dismiss() }
            }

            persistentIndicators

            // Sits under the chrome so the menu's own rows stay tappable, and
            // over the page so a tap outside closes the menu instead of
            // turning a page behind it.
            if showMenu {
                Color.clear
                    .contentShape(Rectangle())
                    .ignoresSafeArea()
                    .onTapGesture { closeMenu() }
            }

            chrome

            // No menu of our own for a live selection: iOS raises its own
            // callout for that, and `AnnotatingWebView` adds Highlight / Note /
            // Remove to it. Two menus for one gesture is what made selecting
            // text feel like a fight.
            if let tap = controller.tappedAnnotation, let highlight = tappedHighlight(tap) {
                // One tap anywhere closes it — including the gutters, which
                // would otherwise turn the page out from under a menu that is
                // about a passage on it. Dismissal is a SwiftUI concern while
                // this is up; the glue never sees these touches.
                Color.clear
                    .contentShape(Rectangle())
                    .ignoresSafeArea()
                    .onTapGesture { closeAnnotationMenu() }

                AnnotationAnchor(rect: tap.rect) {
                    AnnotationMenu(
                        current: highlight.color,
                        hasNote: highlight.note?.nilIfBlank != nil,
                        onColor: { color in
                            Task { await recolor(highlight, to: color) }
                        },
                        onNote: {
                            noteTarget = highlight
                            closeAnnotationMenu()
                        },
                        onCopy: {
                            UIPasteboard.general.string = highlight.text ?? ""
                            closeAnnotationMenu()
                        },
                        onShare: {
                            ShareSheet.present(items: [highlight.text ?? ""])
                            closeAnnotationMenu()
                        },
                        onRemove: {
                            Task { await removeHighlight(highlight) }
                        }
                    )
                }
                // Keyed on the passage so moving to another highlight fades in
                // at the new one rather than sliding the menu across the page.
                .id(tap.cfiRange)
                .transition(.opacity.combined(with: .scale(scale: 0.96)))
            }
        }
        .animation(Motion.snap, value: controller.tappedAnnotation?.cfiRange)
        .statusBarHidden(!chromeVisible)
        .persistentSystemOverlays(chromeVisible ? .automatic : .hidden)
        // The status bar sits on the page, so it has to read against the
        // reading theme — white-on-white otherwise. Sheets opt back out below.
        .preferredColorScheme(pageScheme)
        .task { await prepare() }
        // The glue owns tap zones and swipe-to-turn inside the page; a centre
        // tap arrives here as a token rather than us layering a gesture view
        // over the web view and starving it of touches.
        .onChange(of: controller.chromeToggleToken) { _, _ in
            withAnimation(Motion.settle) {
                chromeVisible.toggle()
                // Hiding the chrome takes the menu with it — it hangs off the
                // button that just left.
                if !chromeVisible { showMenu = false }
            }
        }
        .onChange(of: controller.menuActionToken) { _, _ in
            guard let action = controller.menuAction else { return }
            Task { await perform(action) }
        }
        .onChange(of: controller.location?.cfi) { _, _ in
            Task { await persist(force: false) }
        }
        .onChange(of: bridge.pendingCFI) { _, cfi in
            guard let cfi else { return }
            controller.display(cfi)
            bridge.pendingCFI = nil
        }
        .onDisappear {
            Task { await finish() }
        }
        // The sheets are app surfaces, not page surfaces, so they stay on the
        // app's appearance even when the page is set to Light or Sepia.
        .sheet(isPresented: $showSettings) {
            ReaderSettingsSheet(controller: controller)
                .preferredColorScheme(appScheme)
        }
        .sheet(isPresented: $showContents) {
            ReaderContentsSheet(
                book: book,
                controller: controller,
                highlights: highlights,
                initialTab: contentsTab,
                onRemoveHighlight: { highlight in
                    Task { await removeHighlight(highlight) }
                },
                onBookmarkCountChanged: { bookmarkCount = $0 }
            )
            .preferredColorScheme(appScheme)
        }
        .sheet(item: $noteTarget) { highlight in
            NoteComposer(quote: highlight.text, existing: highlight.note) { note in
                Task { await saveNote(note, on: highlight) }
            }
            .preferredColorScheme(appScheme)
        }
    }

    /// The stored highlight a tapped mark belongs to.
    private func tappedHighlight(_ tap: AnnotationTapData) -> Highlight? {
        highlights.first { $0.epubCFIRange == tap.cfiRange }
    }

    private func closeAnnotationMenu() {
        withAnimation(Motion.snap) { controller.tappedAnnotation = nil }
    }

    private var readerPage: Color {
        switch controller.settings.theme {
        case "light": Palette.light.readerPage
        case "sepia": Palette.sepia.readerPage
        case "black": Palette.black.readerPage
        default: Palette.atrium.readerPage
        }
    }

    /// The two labels, which stay put whether the buttons are showing or not.
    ///
    /// They sit in the bands `#stage` reserves via `env(safe-area-inset-*)`, so
    /// they never overlap prose, and they take no touches so the page keeps
    /// every gesture. Only the buttons come and go on a centre tap — the
    /// position is quiet enough to live there permanently.
    private var persistentIndicators: some View {
        VStack(spacing: 0) {
            indicator(chapterLabel)
                .padding(.top, Spacing.xs)

            Spacer(minLength: 0)

            indicator(positionLabel)
                .padding(.bottom, Spacing.xs)
        }
        .allowsHitTesting(false)
    }

    /// Given the button's own box rather than its own text height, so a label
    /// sits centred on the line its buttons are on however the type resolves.
    @ViewBuilder
    private func indicator(_ label: String?) -> some View {
        if let label {
            Text(label)
                .font(.ui(12.5))
                .foregroundStyle(barInk.opacity(0.5))
                .lineLimit(1)
                .frame(height: ReaderMenu.buttonSize)
                .padding(.horizontal, 72)
        }
    }

    /// How much further to a natural place to stop — the question you have with
    /// a book open, which a raw page number can't answer. Falls back to the
    /// chapter's own position until epub.js's whole-book locations pass lands.
    private var chapterLabel: String? {
        guard let location = controller.location else { return nil }
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

    /// Page numbers need epub.js's whole-book locations pass, which lands
    /// seconds after the first paint; percent stands in until then rather than
    /// a placeholder that jumps.
    private var positionLabel: String? {
        guard let location = controller.location else { return nil }
        if location.hasPageNumbers {
            return "\(location.page) of \(location.totalPages)"
        }
        return location.pct > 0 ? "\(location.pct)%" : nil
    }

    @ViewBuilder
    private var chrome: some View {
        if chromeVisible {
            VStack {
                topBar
                Spacer()
                bottomBar
            }
            .transition(.opacity)
            // Glass resolves against the environment's scheme, not the pixels
            // behind it, so app-dark chrome lands as a grey slab on a white
            // page. Same reason `barInk` follows the reading theme.
            .environment(\.colorScheme, pageScheme)
        }
    }

    /// The scheme the *page* is in, which is what the chrome sits on.
    private var pageScheme: ColorScheme {
        switch controller.settings.theme {
        case "light", "sepia": .light
        default: .dark
        }
    }

    /// The scheme the rest of the app is in.
    private var appScheme: ColorScheme { appState.theme.colorScheme }

    private var topBar: some View {
        HStack {
            if let returnPoint {
                ReaderReturnButton(page: returnPoint.page, ink: barInk) {
                    controller.display(returnPoint.cfi)
                    withAnimation(Motion.snap) { self.returnPoint = nil }
                }
                .transition(.opacity.combined(with: .scale(scale: 0.9)))
            }
            Spacer()
            ReaderGlassButton(
                icon: "xmark",
                label: "Close book",
                ink: barInk,
                diameter: ReaderMenu.buttonSize
            ) {
                dismiss()
            }
        }
        .padding(.horizontal, ReaderMenu.inset)
        .padding(.top, Spacing.xs)
        .animation(Motion.snap, value: returnPoint)
    }

    /// The menu, and the one button that opens it.
    ///
    /// Everything the reader can do that isn't turning a page lives here, so
    /// the page itself carries a single control instead of a row of them.
    private var bottomBar: some View {
        VStack(alignment: .trailing, spacing: ReaderMenu.spacing) {
            if scrubFraction != nil {
                ReaderScrubReadout(
                    chapter: scrubChapter?.label,
                    page: scrubPage,
                    ink: barInk
                )
                .transition(.opacity.combined(with: .scale(scale: 0.96)))
            }

            if showMenu {
                menuRows
            }

            HStack {
                Spacer()
                if showMenu {
                    quickActions
                } else {
                    ReaderGlassButton(
                        icon: "line.3.horizontal",
                        label: "Menu",
                        ink: barInk,
                        diameter: ReaderMenu.buttonSize
                    ) {
                        withAnimation(Motion.lift) { showMenu = true }
                    }
                    .transition(.opacity.combined(with: .scale(scale: 0.9)))
                }
            }
        }
        .padding(.horizontal, ReaderMenu.inset)
        // Closed, the button shares the line with the page count. Open, the
        // whole stack lifts so the quick actions clear it rather than crowding
        // the one label that should always be readable.
        .padding(.bottom, showMenu ? 34 : Spacing.xs)
        .animation(Motion.snap, value: scrubFraction == nil)
    }

    /// The contents entry a scrub is currently pointing at.
    private var scrubChapter: TocItem? {
        guard let fraction = scrubFraction else { return nil }
        return controller.tocEntry(atFraction: fraction)
    }

    /// The page a scrub would land on. Whole-book page numbers only exist once
    /// epub.js's locations pass has run, so this is absent until then.
    private var scrubPage: Int? {
        guard let fraction = scrubFraction,
              let total = controller.location?.totalPages, total > 0
        else { return nil }
        return max(1, min(total, Int((fraction * Double(total)).rounded())))
    }

    @ViewBuilder
    private var menuRows: some View {
        ReaderScrubRow(
            fraction: controller.location?.fraction ?? 0,
            ink: barInk,
            onOpen: {
                closeMenu()
                contentsTab = .contents
                showContents = true
            },
            onScrubStart: markReturnPoint,
            onScrubChange: { scrubFraction = $0 },
            onSeek: { controller.seek(toFraction: $0) }
        )
        .transition(.opacity.combined(with: .move(edge: .bottom)))

        ReaderMenuRow(
            title: "Bookmarks & Highlights",
            count: bookmarkCount + highlights.count,
            ink: barInk
        ) {
            closeMenu()
            contentsTab = .bookmarks
            showContents = true
        }
        .transition(.opacity.combined(with: .move(edge: .bottom)))

        ReaderMenuRow(title: "Themes & Settings", icon: "textformat.size", ink: barInk) {
            closeMenu()
            showSettings = true
        }
        .transition(.opacity.combined(with: .move(edge: .bottom)))
    }

    private var quickActions: some View {
        HStack(spacing: ReaderMenu.spacing) {
            ReaderGlassButton(
                icon: "square.and.arrow.up", label: "Share", ink: barInk, diameter: 50
            ) {
                closeMenu()
                ShareSheet.present(items: [book.displayTitle])
            }
            ReaderGlassButton(
                icon: justBookmarked ? "bookmark.fill" : "bookmark",
                label: "Add bookmark",
                ink: barInk,
                diameter: 50
            ) {
                Task { await addBookmark() }
            }
        }
        .transition(.opacity.combined(with: .scale(scale: 0.9)))
    }

    private func closeMenu() {
        withAnimation(Motion.snap) { showMenu = false }
    }

    /// Remember where a scrub began so the return pill has somewhere to go.
    private func markReturnPoint() {
        guard let location = controller.location, let cfi = location.cfi else { return }
        returnPoint = ReturnPoint(cfi: cfi, page: location.page)
    }

    /// The bars sit on the page ground, not the app ground, so their ink has
    /// to follow the reading theme rather than the app theme.
    private var barInk: Color {
        switch controller.settings.theme {
        case "light", "sepia": Palette.light.ink0Color
        default: Palette.atrium.ink0Color
        }
    }

    // MARK: - Lifecycle

    private func prepare() async {
        guard !didConfigure else { return }
        highlights = await UserDataService.highlights(uuid: book.uuid)
        bookmarkCount = await UserDataService.bookmarks(uuid: book.uuid).count
        if let record = await UserDataService.progress(uuid: book.uuid),
           record.format == .epub {
            startCFI = record.epubCFI
        }
        controller.configure(book: book, startCFI: startCFI, highlights: highlights)
        didConfigure = true
        sessionStart = Date()
    }

    private func persist(force: Bool) async {
        guard let cfi = controller.location?.cfi else { return }
        guard force || Date().timeIntervalSince(lastSave) > 4 else { return }
        lastSave = Date()
        await UserDataService.saveProgress(
            ProgressUpdate(bookUUID: book.uuid, format: .epub, epubCFI: cfi, audioPositionSeconds: nil)
        )
    }

    private func finish() async {
        await persist(force: true)
        let elapsed = Int64(Date().timeIntervalSince(sessionStart))
        if elapsed >= 5 {
            await UserDataService.reportSessions([
                SessionReport(
                    bookUUID: book.uuid,
                    format: .epub,
                    startedAt: Int64(sessionStart.timeIntervalSince1970),
                    endedAt: Int64(Date().timeIntervalSince1970),
                    progressUnits: elapsed,
                    deviceId: nil
                )
            ])
        }
        controller.teardown()
        Presentation.shared.noteProgressPersisted()
    }

    private func addBookmark() async {
        guard let cfi = controller.location?.cfi else { return }
        Haptics.success()
        withAnimation(Motion.snap) { justBookmarked = true }
        bookmarkCount += 1
        await UserDataService.createBookmark(
            CreateBookmark(
                bookUUID: book.uuid, position: cfi, title: controller.location?.chapterName
            )
        )
        // Long enough to read as a confirmation, short enough that the ribbon
        // isn't left claiming this page is permanently marked.
        try? await Task.sleep(for: .seconds(1.2))
        withAnimation(Motion.snap) { justBookmarked = false }
    }

    @discardableResult
    private func createHighlight(
        _ selection: SelectionData, color: HighlightColor
    ) async -> Highlight? {
        controller.addAnnotation(cfiRange: selection.cfiRange, color: color)
        controller.clearSelection()
        Haptics.success()
        let created = await UserDataService.createHighlight(
            CreateHighlight(
                bookUUID: book.uuid,
                epubCFIRange: selection.cfiRange,
                color: color,
                text: selection.text
            )
        )
        // Keep the local list authoritative so the passage is tappable
        // immediately, without waiting for a refetch.
        if let created {
            highlights.append(created)
        }
        return created
    }

    /// Noting a fresh selection highlights it first — a note has to hang on
    /// something, and Apple Books likewise turns "Note" into a highlight.
    private func createHighlightThenNote(_ selection: SelectionData) async {
        guard let created = await createHighlight(selection, color: .amber) else { return }
        noteTarget = created
    }

    /// Run a verb invoked from the system edit menu against the live selection.
    private func perform(_ action: SelectionMenuAction) async {
        guard let selection = controller.selection else { return }
        let rect = selection.rect

        switch action {
        case .highlight:
            guard let created = await createHighlight(selection, color: .amber) else { return }
            // Highlighting opens our own menu on the new mark, so the colour
            // is one tap away rather than fixed at whatever we defaulted to —
            // the same follow-through Apple Books gives the action.
            withAnimation(Motion.snap) {
                controller.tappedAnnotation = AnnotationTapData(
                    cfiRange: created.epubCFIRange, rect: rect
                )
            }

        case .note:
            await createHighlightThenNote(selection)

        case .removeHighlight:
            guard let cfi = selection.existing,
                  let existing = highlights.first(where: { $0.epubCFIRange == cfi })
            else { return }
            controller.clearSelection()
            await removeHighlight(existing)
        }
    }

    private func recolor(_ highlight: Highlight, to color: HighlightColor) async {
        controller.tappedAnnotation = nil
        controller.removeAnnotation(cfiRange: highlight.epubCFIRange)
        controller.addAnnotation(
            cfiRange: highlight.epubCFIRange,
            color: color,
            hasNote: highlight.note?.nilIfBlank != nil
        )
        update(highlight) { $0.color = color }
        await UserDataService.setHighlightColor(
            id: highlight.id, color: color, bookUUID: book.uuid
        )
    }

    private func saveNote(_ note: String?, on highlight: Highlight) async {
        // Re-add the mark so the note underline appears or clears to match.
        controller.removeAnnotation(cfiRange: highlight.epubCFIRange)
        controller.addAnnotation(
            cfiRange: highlight.epubCFIRange,
            color: highlight.color,
            hasNote: note != nil
        )
        update(highlight) { $0.note = note }
        Haptics.success()
        await UserDataService.setHighlightNote(
            id: highlight.id, note: note, bookUUID: book.uuid
        )
    }

    private func removeHighlight(_ highlight: Highlight) async {
        controller.tappedAnnotation = nil
        controller.removeAnnotation(cfiRange: highlight.epubCFIRange)
        highlights.removeAll { $0.id == highlight.id }
        Haptics.warning()
        await UserDataService.deleteHighlight(id: highlight.id, bookUUID: book.uuid)
    }

    private func update(_ highlight: Highlight, _ change: (inout Highlight) -> Void) {
        guard let index = highlights.firstIndex(where: { $0.id == highlight.id }) else { return }
        change(&highlights[index])
    }
}

// MARK: - Settings sheet

/// Reading typography.
///
/// Was a stock `Form` of system segmented controls — grey chrome from a
/// different app, on the surface where the reader is most likely to be looking
/// closely at how things are set. Themes are now shown in the page colours they
/// actually produce, so picking one is looking rather than guessing.
private struct ReaderSettingsSheet: View {
    @Bindable var controller: ReaderController

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    private let themes: [(token: String, label: String)] = [
        ("light", "Light"), ("sepia", "Sepia"), ("dark", "Dark"), ("black", "Black"),
    ]
    private let fonts: [(token: String, label: String)] = [
        ("serif", "Serif"), ("sans-serif", "Sans"), ("monospace", "Mono"),
    ]

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 26) {
                    group("Page") {
                        HStack(spacing: Spacing.md) {
                            ForEach(themes, id: \.token) { theme in
                                PageSwatch(
                                    token: theme.token,
                                    label: theme.label,
                                    isOn: controller.settings.theme == theme.token
                                ) {
                                    guard controller.settings.theme != theme.token else { return }
                                    Haptics.select()
                                    withAnimation(Motion.settle) {
                                        controller.settings.theme = theme.token
                                    }
                                }
                            }
                        }
                    }

                    group("Type") {
                        PillSelector(
                            options: fonts.map(\.token),
                            label: { token in
                                fonts.first { $0.token == token }?.label ?? token
                            },
                            selection: $controller.settings.fontFamily
                        )

                        Plate {
                            sizeRow
                            lineHeightRow
                        }
                    }

                    group("Layout") {
                        PillSelector(
                            options: ReaderMargins.allCases,
                            label: \.label,
                            selection: $controller.settings.margins
                        )

                        Plate {
                            PlateRow(label: "Justify text", isFirst: true) {
                                Toggle("", isOn: $controller.settings.justify)
                                    .labelsHidden()
                                    .tint(palette.accentColor)
                            }
                        }
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 32)
            }
            .scrollIndicators(.hidden)
            .background(ScreenBackground())
            .navigationTitle("Reading")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        // Tall enough that every control is on screen at rest — a `.medium`
        // detent cut the layout group off, so half the settings needed a drag
        // to discover — while still leaving page enough of the page visible to
        // watch a change land.
        .presentationDetents([.height(600), .large])
        .tint(palette.accentColor)
    }

    /// The size is set in the face it controls, so the control previews itself.
    private var sizeRow: some View {
        PlateRow(label: "Size", isFirst: true) {
            HStack(spacing: Spacing.md) {
                Text("Aa")
                    .font(.system(size: CGFloat(controller.settings.fontSize), design: .serif))
                    .foregroundStyle(palette.ink1Color)
                    .frame(width: 46, alignment: .trailing)
                    .animation(Motion.snap, value: controller.settings.fontSize)

                stepButton("minus", enabled: controller.settings.fontSize > 12) {
                    controller.settings.fontSize = max(12, controller.settings.fontSize - 1)
                }
                stepButton("plus", enabled: controller.settings.fontSize < 34) {
                    controller.settings.fontSize = min(34, controller.settings.fontSize + 1)
                }
            }
        }
    }

    private var lineHeightRow: some View {
        VStack(spacing: 0) {
            Hairline()

            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    RowLabel("Line height")
                    Spacer(minLength: 0)
                    Text(String(format: "%.1f", controller.settings.lineHeight))
                        .font(.monoUI(12))
                        .foregroundStyle(palette.ink2Color)
                        .contentTransition(.numericText())
                }
                Slider(value: $controller.settings.lineHeight, in: 1.2...2.2, step: 0.1)
                    .tint(palette.accentColor)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
        }
    }

    private func stepButton(
        _ icon: String, enabled: Bool, action: @escaping () -> Void
    ) -> some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(enabled ? palette.accentColor : palette.ink3Color)
                .frame(width: 32, height: 32)
                .background(Circle().fill(palette.bg2Color))
                .overlay(Circle().strokeBorder(palette.line2.color, lineWidth: 0.5))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private func group<Content: View>(
        _ title: String, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel(title)
            content()
        }
    }
}

/// One reading theme, shown as the page it produces.
private struct PageSwatch: View {
    let token: String
    let label: String
    let isOn: Bool
    let action: () -> Void

    @Environment(\.palette) private var palette

    private var page: Color {
        switch token {
        case "light": Palette.light.readerPage
        case "sepia": Palette.sepia.readerPage
        case "black": Palette.black.readerPage
        default: Palette.atrium.readerPage
        }
    }

    private var ink: Color {
        token == "light" || token == "sepia"
            ? Palette.light.ink0Color
            : Palette.atrium.ink0Color
    }

    var body: some View {
        Button(action: action) {
            VStack(spacing: 7) {
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(page)
                    .aspectRatio(0.82, contentMode: .fit)
                    .overlay {
                        VStack(alignment: .leading, spacing: 3.5) {
                            ForEach([1.0, 0.86, 0.94, 0.6], id: \.self) { fraction in
                                Capsule()
                                    .fill(ink.opacity(0.6))
                                    .frame(width: 30 * fraction, height: 2.5)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .center)
                    }
                    .overlay(
                        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                            .strokeBorder(
                                isOn ? palette.accentColor : palette.lineColor,
                                lineWidth: isOn ? 2 : 0.75
                            )
                    )

                Text(label)
                    .font(.ui(11, weight: isOn ? .semibold : .regular))
                    .foregroundStyle(isOn ? palette.ink0Color : palette.ink3Color)
            }
        }
        .buttonStyle(PressableStyle())
        .accessibilityLabel(label)
        .accessibilityAddTraits(isOn ? [.isSelected, .isButton] : .isButton)
    }
}
