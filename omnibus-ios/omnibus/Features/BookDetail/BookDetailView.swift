//  BookDetailView.swift
//  The book hero and everything hanging off it: read/listen actions, rating,
//  read status, download, shelves, journal, annotations, and metadata.

import SwiftUI

@Observable
@MainActor
final class BookDetailModel {
    var book: Book?
    var rating: Double = 0
    var otherRatings: [AttributedRating] = []
    var readStatus: ReadStatus = .unread
    var journals: [JournalEntry] = []
    var highlights: [Highlight] = []
    var bookmarks: [Bookmark] = []
    var shelvesContaining: Set<Int64> = []
    var isLoading = true
    var error: String?

    /// Every section is its own live read, so each paints from the replica at
    /// once and updates independently if the server disagrees. Nothing here
    /// waits on anything else — a slow endpoint can no longer hold the page.
    func load(uuid: String) async {
        isLoading = book == nil

        await withTaskGroup(of: Void.self) { group in
            group.addTask { @MainActor in
                do {
                    for try await read in LibraryService.book(uuid: uuid) {
                        self.book = read.value
                        self.error = nil
                        self.isLoading = false
                    }
                } catch {
                    if self.book == nil {
                        self.error = (error as? APIError)?.errorDescription
                            ?? error.localizedDescription
                    }
                }
                self.isLoading = false
            }
            group.addTask { @MainActor in
                for await record in UserDataService.rating(uuid: uuid).values() {
                    self.rating = record.stars
                }
            }
            group.addTask { @MainActor in
                for await others in UserDataService.otherRatings(uuid: uuid).values() {
                    self.otherRatings = others
                }
            }
            group.addTask { @MainActor in
                for await record in UserDataService.readStatus(uuid: uuid).values() {
                    self.readStatus = record.status
                }
            }
            group.addTask { @MainActor in
                for await entries in UserDataService.journals(uuid: uuid).values() {
                    self.journals = entries
                }
            }
            group.addTask { @MainActor in
                for await items in UserDataService.highlights(uuid: uuid).values() {
                    self.highlights = items
                }
            }
            group.addTask { @MainActor in
                for await items in UserDataService.bookmarks(uuid: uuid).values() {
                    self.bookmarks = items
                }
            }
            group.addTask { @MainActor in
                for await ids in UserDataService.shelvesContaining(uuid: uuid).values() {
                    self.shelvesContaining = Set(ids)
                }
            }
        }
    }

    func setRating(_ stars: Double, uuid: String) async {
        rating = stars
        await UserDataService.setRating(uuid: uuid, stars: stars)
    }

    func clearRating(uuid: String) async {
        rating = 0
        await UserDataService.clearRating(uuid: uuid)
    }

    func setStatus(_ status: ReadStatus, uuid: String) async {
        readStatus = status
        await UserDataService.setReadStatus(uuid: uuid, status: status)
    }
}

struct BookDetailView: View {
    let uuid: String

    init(uuid: String) {
        self.uuid = uuid
    }

    @Environment(\.palette) private var palette
    @Environment(AppState.self) private var app
    @Environment(\.bookZoomNamespace) private var bookZoom
    @State private var model = BookDetailModel()
    @State private var showJournalComposer = false
    @State private var showShelfPicker = false
    @State private var showBookmarks = false
    @State private var showAudioFilePicker = false
    /// The file chosen in the picker, opened from the sheet's `onDismiss` —
    /// presenting the full-screen player while the sheet is still up would
    /// be refused.
    @State private var pickedAudioFile: BookFileInfo?
    @State private var showDeleteConfirm = false
    @State private var scrollY: CGFloat = 0
    @State private var aboutExpanded = false
    /// Non-nil when the composer is open on an existing entry.
    @State private var editingJournal: JournalEntry?

    private var downloads = DownloadManager.shared

    /// Reflects membership so the row states what tapping it will do.
    private var shelfButtonLabel: String {
        let count = model.shelvesContaining.count
        switch count {
        case 0: return "Add to shelf"
        case 1: return "On 1 shelf"
        default: return "On \(count) shelves"
        }
    }

    var body: some View {
        Group {
            if model.isLoading {
                LoadingView()
            } else if let book = model.book {
                content(book)
            } else if let error = model.error {
                ErrorStateView(message: error) { Task { await model.load(uuid: uuid) } }
            }
        }
        .background(ScreenBackground())
        // The hero already sets the title at full size; repeating it in the bar
        // 40pt above is noise. It fades in only once the real one scrolls away.
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        // The hero gradient runs to the top edge; an opaque bar would cut a
        // hard band across it. What scrolls under the bar stays legible via the
        // scroll edge effect plus `BookHero` dissolving the jacket.
        .toolbarBackground(.hidden, for: .navigationBar)
        .toolbar { toolbar }
        .bookZoomDestination(uuid, in: bookZoom)
        .task { await model.load(uuid: uuid) }
        .refreshable { await model.load(uuid: uuid) }
        .sheet(isPresented: $showJournalComposer) {
            if let book = model.book {
                JournalComposer(book: book, editing: editingJournal) {
                    Task { await model.load(uuid: uuid) }
                }
            }
        }
        .sheet(isPresented: $showShelfPicker, onDismiss: {
            Task {
                for await ids in UserDataService.shelvesContaining(uuid: uuid).values() {
                    model.shelvesContaining = Set(ids)
                }
            }
        }) {
            if let book = model.book {
                ShelfPickerSheet(book: book)
            }
        }
        .sheet(isPresented: $showBookmarks) {
            if let book = model.book {
                BookmarksSheet(book: book, isAudio: book.hasAudiobook)
            }
        }
        .sheet(isPresented: $showAudioFilePicker, onDismiss: openPickedAudioFile) {
            if let book = model.book {
                AudioFilePickerSheet(book: book) { file in
                    pickedAudioFile = file
                }
            }
        }
    }

    /// Open the player on the file the picker chose, once its sheet is gone.
    private func openPickedAudioFile() {
        guard let file = pickedAudioFile, let book = model.book else { return }
        pickedAudioFile = nil
        Presentation.shared.openPlayer(book, fileID: file.id)
    }

    private func content(_ book: Book) -> some View {
        // The wash is pinned behind the scroll view rather than living inside
        // the scrolled content: a background inside the content stops at the
        // safe area and leaves a black slab under the navigation bar. The
        // sections below the hero carry an opaque ground so they occlude it
        // cleanly as the page scrolls.
        ZStack(alignment: .top) {
            BookHeroWash(tone: accent(book))
                .frame(height: 620)
                .frame(maxHeight: .infinity, alignment: .top)
                .ignoresSafeArea(edges: .top)

            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    BookHero(book: book, scrollY: scrollY)

                    // What the book *is* leads; how you've filed it follows.
                    // Someone on this screen has already chosen not to be
                    // reading, so the page is paced for looking rather than
                    // packed for reach.
                    VStack(alignment: .leading, spacing: 34) {
                        actions(book)
                        if let description = book.description?.nilIfBlank {
                            aboutSection(description)
                        }
                        if !book.subjects.isEmpty { tagsSection(book) }
                        ratingSection(book)
                        statusSection(book)
                        librarySection(book)
                        metadataSection(book)
                        if !model.highlights.isEmpty { highlightsSection(book) }
                        journalSection(book)
                        if !model.otherRatings.isEmpty { otherRatingsSection }
                    }
                    .screenPadding()
                    .padding(.top, Spacing.xl)
                    .padding(.bottom, 48)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(palette.bg0Color)
                }
            }
            .scrollIndicators(.hidden)
            // Do NOT hide the top scroll edge effect here: the accent-filled
            // Read/Listen button scrolls directly under the transparent bar,
            // and the effect's blur is the only thing separating the bar title
            // from it. Without it, the button's own label reads through the
            // title.
            .onScrollGeometryChange(for: CGFloat.self) { geometry in
                geometry.contentOffset.y + geometry.contentInsets.top
            } action: { _, offset in
                scrollY = offset
            }

            barScrim(book)
        }
    }

    /// An opaque band behind the navigation bar, faded in as the hero leaves.
    ///
    /// The scroll edge effect blurs what passes under the bar but doesn't
    /// occlude it, and what passes under here is a full-width accent button —
    /// blurred, its label still reads straight through the bar title. The band
    /// is the wash's own top colour, so at rest it is invisible against the
    /// hero and only starts covering once there is something to cover.
    private func barScrim(_ book: Book) -> some View {
        let tone = accent(book)
        let isLight = palette.bg0.l > 0.5
        let top = OKLCH(isLight ? 0.92 : 0.26, tone.c * 0.7, tone.h).color

        return LinearGradient(
            stops: [
                .init(color: top, location: 0),
                // Opaque past the bar's own bottom edge, then a long fade —
                // a short ramp put a visible seam across the page.
                .init(color: top, location: 0.68),
                .init(color: top.opacity(0.45), location: 0.85),
                .init(color: top.opacity(0), location: 1),
            ],
            startPoint: .top,
            endPoint: .bottom
        )
        .frame(height: 158)
        .frame(maxHeight: .infinity, alignment: .top)
        .ignoresSafeArea(edges: .top)
        .opacity(barTitleOpacity)
        .allowsHitTesting(false)
    }

    // MARK: - Hero

    /// The book's tone, resolved the same way the cover plate resolves it, so
    /// a coverless book's wash matches the plate it is showing.
    private func accent(_ book: Book) -> OKLCH {
        CoverIdentity(book).tone
    }

    /// Fades the bar title in only once the hero's own title has passed under
    /// the bar, so the two are never both on screen.
    private var barTitleOpacity: Double {
        Double(min(1, max(0, (scrollY - 300) / 50)))
    }

    // MARK: - Primary actions

    private func actions(_ book: Book) -> some View {
        VStack(spacing: Spacing.sm) {
            HStack(spacing: Spacing.sm) {
                if book.hasEbook {
                    Button {
                        Haptics.tap()
                        Presentation.shared.openReader(book)
                    } label: {
                        Label("Read", systemImage: "book")
                    }
                    .buttonStyle(FilledButtonStyle())
                }
                if book.hasAudiobook {
                    Button {
                        Haptics.tap()
                        listen(book)
                    } label: {
                        Label("Listen", systemImage: "headphones")
                    }
                    .buttonStyle(FilledButtonStyle(prominent: !book.hasEbook))
                }
            }
        }
    }

    /// A book with one audiobook file opens straight into the player; one
    /// with several asks which — they don't share a timeline, so "Listen"
    /// alone can't say where to put the needle. The player resolves the
    /// saved-position file on its own for the single-file case.
    private func listen(_ book: Book) {
        if book.audioFiles.count > 1 {
            showAudioFilePicker = true
        } else {
            Presentation.shared.openPlayer(book)
        }
    }

    // MARK: - About

    /// The jacket blurb, set as prose.
    ///
    /// A raised initial and the reading face are the whole point: this is the
    /// only stretch of real writing on the screen, and it was previously
    /// folded behind a disclosure chevron below the admin controls.
    private func aboutSection(_ description: String) -> some View {
        let text = description.strippingHTML

        return VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("About")

            Text(setBlurb(text))
                .lineSpacing(6)
                .lineLimit(aboutExpanded ? nil : 9)
                .fixedSize(horizontal: false, vertical: true)

            if isClamped(text) {
                Button {
                    withAnimation(Motion.settle) { aboutExpanded.toggle() }
                } label: {
                    Text(aboutExpanded ? "Show less" : "Read more")
                        .font(.ui(13, weight: .semibold))
                        .foregroundStyle(palette.accentColor)
                }
                .buttonStyle(.plain)
            }
        }
    }

    /// The blurb, with its opening letter set large and in the accent when
    /// there's a paragraph to hang it on.
    ///
    /// Not every book has a jacket blurb: an audiobook with no metadata gets a
    /// synthesised one-liner ("Audiobook · 0h 06m"), and a raised initial on
    /// that reads as a rendering fault rather than as typography.
    private func setBlurb(_ text: String) -> AttributedString {
        var body = AttributedString(isProse(text) ? String(text.dropFirst()) : text)
        body.font = .display(17.5)
        body.foregroundColor = palette.ink1Color

        guard isProse(text) else { return body }

        var initial = AttributedString(String(text.prefix(1)))
        initial.font = .system(size: 44, weight: .regular, design: .serif)
        initial.foregroundColor = palette.accentColor
        return initial + body
    }

    /// Long enough to read as writing, and opening on a letter the initial can
    /// actually be set from.
    private func isProse(_ text: String) -> Bool {
        text.count >= 140 && (text.first?.isLetter ?? false)
    }

    /// Whether the nine-line clamp is doing anything. Offering "Read more" on a
    /// blurb that already fits leaves a control that visibly does nothing.
    private func isClamped(_ text: String) -> Bool {
        text.count > 330
    }

    // MARK: - Rating & status

    /// Rating gets its own band rather than a plate row: it's the one control
    /// on the page the reader is expected to *use*, and a drag needs room.
    private func ratingSection(_ book: Book) -> some View {
        VStack(spacing: Spacing.md) {
            SectionLabel("Your rating")

            StarRating(stars: model.rating, size: 32, interactive: true) { stars in
                Task { await model.setRating(stars, uuid: book.uuid) }
            }
            .frame(height: 38)

            HStack(spacing: Spacing.sm) {
                if model.rating > 0 {
                    Text(model.rating.formatted())
                        .font(.display(19))
                        .foregroundStyle(palette.ink1Color)
                        .contentTransition(.numericText())

                    Button("Clear") {
                        Task { await model.clearRating(uuid: book.uuid) }
                    }
                    .font(.ui(12.5, weight: .medium))
                    .foregroundStyle(palette.ink3Color)
                } else {
                    Text("Drag across the stars to rate")
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink3Color)
                }
            }
            .animation(Motion.snap, value: model.rating)
        }
        .frame(maxWidth: .infinity)
    }

    private func statusSection(_ book: Book) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Status")

            PillSelector(
                options: ReadStatus.allCases,
                label: \.label,
                selection: Binding(
                    get: { model.readStatus },
                    set: { model.readStatus = $0 }
                )
            ) { status in
                Task { await model.setStatus(status, uuid: book.uuid) }
            }
        }
    }

    /// Shelving and offline state.
    private func librarySection(_ book: Book) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            SectionLabel("In your library")

            VStack(spacing: 0) {
                RecordRow(label: "Shelves", isFirst: true) {
                    Button {
                        Haptics.tap()
                        showShelfPicker = true
                    } label: {
                        HStack(spacing: 5) {
                            Text(shelfButtonLabel)
                                .font(.ui(14.5, weight: .medium))
                            Image(systemName: "chevron.right")
                                .font(.system(size: 11, weight: .semibold))
                        }
                        .foregroundStyle(palette.accentColor)
                    }
                    .buttonStyle(.plain)
                }

                RecordRow(label: "Offline") {
                    downloadControl(book)
                }

                RecordRule()
            }
        }
    }

    /// The offline cell's content, sized to sit inline in a plate row.
    ///
    /// A dual-format book gets a control per format: they are two files, either
    /// of which a reader might want on the device, and the registry now holds
    /// them separately. A single-format book still reads as one plain control.
    @ViewBuilder
    private func downloadControl(_ book: Book) -> some View {
        let kinds = DownloadManager.kinds(for: book)
        if kinds.count > 1 {
            VStack(alignment: .trailing, spacing: 6) {
                ForEach(kinds, id: \.self) { kind in
                    HStack(spacing: Spacing.sm) {
                        Text(kind == .audio ? "Audiobook" : "Ebook")
                            .font(.ui(11.5))
                            .foregroundStyle(palette.ink3Color)
                        downloadControl(book, kind: kind)
                    }
                }
            }
        } else {
            downloadControl(book, kind: kinds.first ?? .ebook)
        }
    }

    @ViewBuilder
    private func downloadControl(_ book: Book, kind: DownloadKind) -> some View {
        let record = downloads.record(for: book.uuid, kind: kind)

        switch record?.state {
        case .complete:
            HStack(spacing: Spacing.sm) {
                Text(Format.bytes(record?.totalBytes ?? 0))
                    .font(.monoUI(11))
                    .foregroundStyle(palette.ink3Color)
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 15))
                    .foregroundStyle(palette.okColor)
                Button("Remove") {
                    Task { await downloads.remove(book.uuid, kind: kind) }
                }
                .font(.ui(13))
                .foregroundStyle(palette.badColor)
            }

        case .running, .queued:
            HStack(spacing: Spacing.sm) {
                ProgressView(value: record?.fraction ?? 0)
                    .tint(palette.accentColor)
                    .frame(width: 90)
                Button("Cancel") { Task { await downloads.cancel(book.uuid, kind: kind) } }
                    .font(.ui(13))
                    .foregroundStyle(palette.ink3Color)
            }

        default:
            Button {
                Haptics.tap()
                Task { await downloads.start(book: book, kind: kind) }
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.down.circle")
                        .font(.system(size: 13, weight: .semibold))
                    Text(record?.state == .failed ? "Retry" : "Download")
                        .font(.ui(14, weight: .medium))
                }
                .foregroundStyle(palette.accentColor)
            }
            .buttonStyle(.plain)
        }
    }

    // MARK: - Sections

    private func tagsSection(_ book: Book) -> some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Tags")
            FlowLayout(spacing: 6) {
                ForEach(book.subjects, id: \.self) { subject in
                    NavigationLink(value: Destination.tag(name: subject)) {
                        Chip(label: subject)
                    }
                    .buttonStyle(PressableStyle())
                }
            }
        }
    }

    /// The colophon. Short enough that hiding it behind a disclosure chevron
    /// bought nothing but a control to tap.
    private func metadataSection(_ book: Book) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            SectionLabel("Details")

            VStack(spacing: 0) {
                metaRow("Publisher", book.publisher, isFirst: true)
                metaRow("Published", book.published.map(Format.looseDate))
                metaRow("Language", book.language)
                metaRow("ISBN", book.isbn13)
                metaRow("Added", book.addedAt.map(Format.isoDate))
                metaRow("File", book.filename)
                if let size = book.epubSizeBytes {
                    metaRow("Size", Format.bytes(size))
                }
                RecordRule()
            }
        }
    }

    /// Colophon rows: mono small-caps keys against the reading face, the way a
    /// book states its own printing details.
    @ViewBuilder
    private func metaRow(_ key: String, _ value: String?, isFirst: Bool = false) -> some View {
        if let value = value?.nilIfBlank {
            VStack(spacing: 0) {
                if !isFirst {
                    Rectangle()
                        .fill(palette.line2.color)
                        .frame(height: 0.5)
                }

                HStack(alignment: .firstTextBaseline, spacing: Spacing.md) {
                    Text(key.uppercased())
                        .font(.monoUI(10, weight: .medium))
                        .tracking(0.7)
                        .foregroundStyle(palette.ink3Color)
                        .frame(width: 92, alignment: .leading)

                    Text(value)
                        .font(.ui(14.5))
                        .foregroundStyle(palette.ink1Color)
                        .textSelection(.enabled)

                    Spacer(minLength: 0)
                }
                .padding(.vertical, 13)
            }
        }
    }

    private func highlightsSection(_ book: Book) -> some View {
        DisclosureSection(title: "Highlights (\(model.highlights.count))") {
            VStack(alignment: .leading, spacing: Spacing.md) {
                ForEach(model.highlights.prefix(6)) { highlight in
                    VStack(alignment: .leading, spacing: 4) {
                        if let text = highlight.text?.nilIfBlank {
                            Text(text)
                                .font(.display(15))
                                .foregroundStyle(palette.ink0Color)
                                .lineLimit(4)
                        }
                        if let note = highlight.note?.nilIfBlank {
                            Text(note)
                                .font(.ui(12.5))
                                .foregroundStyle(palette.ink2Color)
                        }
                    }
                    .padding(.leading, Spacing.md)
                    .overlay(alignment: .leading) {
                        Capsule()
                            .fill(highlight.color.tint)
                            .frame(width: 3)
                    }
                }
            }
        }
    }

    private func journalSection(_ book: Book) -> some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            HStack(alignment: .firstTextBaseline) {
                SectionLabel("Journal")

                Spacer(minLength: Spacing.sm)

                Button {
                    Haptics.tap()
                    editingJournal = nil
                    showJournalComposer = true
                } label: {
                    Label("Write", systemImage: "square.and.pencil")
                        .font(.ui(13, weight: .semibold))
                }
                .foregroundStyle(palette.accentColor)
            }

            if model.journals.isEmpty {
                journalEmptyState
            } else {
                VStack(alignment: .leading, spacing: Spacing.xl) {
                    ForEach(model.journals) { entry in
                        JournalCard(entry: entry) {
                            editingJournal = entry
                            showJournalComposer = true
                        } onDelete: {
                            Task {
                                await UserDataService.deleteJournal(entry)
                                await model.load(uuid: book.uuid)
                            }
                        }
                    }
                }
            }
        }
    }

    /// An invitation rather than a status line — this is the one place on the
    /// screen the reader is asked to write something.
    private var journalEmptyState: some View {
        Button {
            Haptics.tap()
            editingJournal = nil
            showJournalComposer = true
        } label: {
            VStack(alignment: .leading, spacing: 5) {
                Text("Start a journal")
                    .font(.display(17))
                    .foregroundStyle(palette.ink1Color)
                Text("Notes, quotes, and what you made of it — kept with the book.")
                    .font(.ui(13))
                    .foregroundStyle(palette.ink3Color)
                    .multilineTextAlignment(.leading)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, Spacing.lg)
            .padding(.horizontal, Spacing.lg)
            .background(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .strokeBorder(
                        palette.line2.color,
                        style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                    )
            )
        }
        .buttonStyle(.plain)
    }

    private var otherRatingsSection: some View {
        DisclosureSection(title: "Other readers") {
            VStack(spacing: Spacing.sm) {
                ForEach(model.otherRatings) { rating in
                    HStack {
                        Text(rating.username)
                            .font(.ui(13.5))
                            .foregroundStyle(palette.ink1Color)
                        Spacer()
                        StarRating(stars: rating.stars)
                    }
                }
            }
        }
    }

    // MARK: - Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .principal) {
            Text(model.book?.displayTitle ?? "")
                .font(.system(size: 16, weight: .medium, design: .serif))
                .foregroundStyle(palette.ink0Color)
                .lineLimit(1)
                .opacity(barTitleOpacity)
        }
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Button { showShelfPicker = true } label: {
                    Label("Add to shelf", systemImage: "square.stack")
                }
                Button { showBookmarks = true } label: {
                    Label("Bookmarks", systemImage: "bookmark")
                }
                if app.user?.canEdit == true, let book = model.book {
                    NavigationLink(value: Destination.metadataEdit(uuid: book.uuid)) {
                        Label("Edit metadata", systemImage: "pencil")
                    }
                }
                if let book = model.book, app.user?.canDownload == true {
                    Button {
                        Task { await shareBook(book) }
                    } label: {
                        Label("Export file", systemImage: "square.and.arrow.up")
                    }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
        }
    }

    private func shareBook(_ book: Book) async {
        // Reuse the offline copy when one exists so an export costs nothing.
        var url = downloads.localURL(for: book.uuid, kind: .ebook)
        if url == nil {
            guard let data = try? await APIClient.shared.data(for: "/api/ebooks/\(book.uuid)/download")
            else { return }
            let temp = FileManager.default.temporaryDirectory
                .appendingPathComponent("\(book.displayTitle).epub")
            try? data.write(to: temp)
            url = temp
        }
        guard let url else { return }
        ShareSheet.present(items: [url])
    }
}

// MARK: - Support views

/// A collapsible section that starts expanded — used for the long-form blocks.
struct DisclosureSection<Content: View>: View {
    let title: String
    @ViewBuilder var content: () -> Content

    @State private var expanded = true
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            Button {
                withAnimation(Motion.settle) { expanded.toggle() }
            } label: {
                HStack {
                    Text(title)
                        .font(.display(20))
                        .foregroundStyle(palette.ink0Color)
                    Spacer()
                    Image(systemName: "chevron.down")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(palette.ink3Color)
                        .rotationEffect(.degrees(expanded ? 0 : -90))
                }
            }
            .buttonStyle(.plain)

            if expanded {
                content()
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }
}

extension HighlightColor {
    var tint: Color {
        switch self {
        case .amber: OKLCH(0.80, 0.14, 80).color
        case .green: OKLCH(0.78, 0.13, 150).color
        case .blue: OKLCH(0.72, 0.12, 250).color
        case .rose: OKLCH(0.72, 0.15, 15).color
        case .violet: OKLCH(0.70, 0.14, 300).color
        }
    }
}

extension String {
    /// Book descriptions arrive as OPF HTML fragments.
    var strippingHTML: String {
        replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression)
            .replacingOccurrences(of: "&nbsp;", with: " ")
            .replacingOccurrences(of: "&amp;", with: "&")
            .replacingOccurrences(of: "&lt;", with: "<")
            .replacingOccurrences(of: "&gt;", with: ">")
            .replacingOccurrences(of: "&#39;", with: "'")
            .replacingOccurrences(of: "&quot;", with: "\"")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
