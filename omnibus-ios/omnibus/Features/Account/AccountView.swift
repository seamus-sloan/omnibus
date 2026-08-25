//  AccountView.swift
//  The "You" tab: identity, appearance, offline storage, and sign-out.
//
//  Was a stock inset-grouped `List` with system section headers, which made the
//  one tab that isn't about books look like it belonged to a different app —
//  grey sans headers against the editorial face everywhere else. It is built
//  from the same masthead, section label, and record-row vocabulary the rest of
//  the app uses.

import SwiftUI

struct AccountView: View {
    @Environment(AppState.self) private var app
    @Environment(\.palette) private var palette

    @State private var showSignOutConfirm = false
    @State private var showAddBooks = false
    @State private var showRejected = false
    @State private var showProfileEdit = false
    @State private var kindleEmail = ""
    @State private var kindleSaved = false
    @State private var kindleError: String?
    @State private var hiddenFormats: Set<String> = []
    @State private var hiddenSaved = false
    @State private var hiddenError: String?
    private var downloads = DownloadManager.shared
    private var connectivity = Connectivity.shared

    private var downloadCount: Int {
        downloads.records.values.filter { $0.state == .complete }.count
    }

    var body: some View {
        @Bindable var app = app

        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 30) {
                    Masthead(title: "You")

                    identityCard

                    appearance($app)
                    library
                    offline
                    sendToKindle
                    hiddenFormatsSection
                    session
                    colophon
                }
                .padding(.bottom, 40)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .background(ScreenBackground())
            .toolbar(.hidden, for: .navigationBar)
            .topEdgeScrim()
            .withDestinations()
            .sheet(isPresented: $showAddBooks) { AddBooksSheet() }
            .sheet(isPresented: $showRejected) { RejectedWritesSheet() }
            .sheet(isPresented: $showProfileEdit) {
                if let user = app.user {
                    ProfileEditSheet(user: user) { await app.refreshUser() }
                }
            }
            .confirmationDialog(
                "Sign out of Omnibus?", isPresented: $showSignOutConfirm, titleVisibility: .visible
            ) {
                Button("Sign out", role: .destructive) {
                    Task { await app.signOut() }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Downloaded books stay on this device.")
            }
        }
        .task {
            kindleEmail = app.user?.kindleEmail ?? ""
            hiddenFormats = Set(app.user?.hiddenFormats ?? [])
            await connectivity.refreshPendingCount()
        }
        .onChange(of: kindleEmail) { _, _ in
            kindleSaved = false
            kindleError = nil
        }
        .onChange(of: hiddenFormats) { _, _ in
            hiddenSaved = false
            hiddenError = nil
        }
    }

    // MARK: - Identity

    /// The account, given the weight a name deserves rather than a 44pt row.
    /// Tapping it opens the profile editor (display name + picture).
    private var identityCard: some View {
        HStack(spacing: Spacing.lg) {
            UserAvatar(
                id: app.user?.id ?? 0,
                name: app.user?.display ?? "?",
                hasAvatar: app.user?.hasAvatar ?? false
            )

            VStack(alignment: .leading, spacing: 3) {
                Text(app.user?.display ?? "Signed in")
                    .font(.display(27))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)

                Text(roleLine)
                    .font(.monoUI(10, weight: .medium))
                    .tracking(0.7)
                    .textCase(.uppercase)
                    .foregroundStyle(app.user?.isAdmin == true ? palette.accentColor : palette.ink3Color)
            }

            Spacer(minLength: 0)
        }
        .screenPadding()
        // The whole row is the target, not just the avatar — the name is as
        // obvious a thing to tap as the picture.
        .contentShape(Rectangle())
        .onTapGesture { if app.user != nil { showProfileEdit = true } }
        .accessibilityIdentifier("identity-card")
        .accessibilityAddTraits(.isButton)
        .accessibilityHint("Edit your profile")
    }

    private var roleLine: String {
        guard let user = app.user else { return "Signed in" }
        return user.isAdmin ? "Administrator" : "Reader"
    }

    // MARK: - Sections

    private func appearance(_ app: Bindable<AppState>) -> some View {
        group("Appearance") {
            // Themes are the one setting whose whole point is how it looks, so
            // each option carries its own ground and ink rather than a label in
            // a menu you have to commit to before you can see it.
            HStack(spacing: Spacing.md) {
                ForEach(ThemeName.allCases, id: \.self) { name in
                    ThemeSwatch(name: name, isOn: app.wrappedValue.theme == name) {
                        guard app.wrappedValue.theme != name else { return }
                        Haptics.select()
                        withAnimation(Motion.settle) { app.wrappedValue.theme = name }
                    }
                }
            }
        }
    }

    private var library: some View {
        group("Library") {
            VStack(spacing: 0) {
                actionRow("Add books", icon: "plus.circle", isFirst: true) {
                    Haptics.tap()
                    showAddBooks = true
                }
                linkRow("Shelves", icon: "square.stack", to: .shelves)
                linkRow("Authors", icon: "person.2", to: .authorsIndex)
                linkRow("Series", icon: "square.grid.2x2", to: .seriesIndex)
                linkRow("Tags", icon: "tag", to: .tags)
                if app.user?.isAdmin == true {
                    linkRow("Server settings", icon: "gearshape", to: .settings)
                }
                RecordRule()
            }
        }
    }

    private var offline: some View {
        group("Offline") {
            VStack(spacing: 0) {
                NavigationLink(value: Destination.downloads) {
                    rowShell(isFirst: true) {
                        icon("arrow.down.circle")
                        rowTitle("Downloads")
                        Spacer(minLength: 0)
                        Text("\(downloadCount)")
                            .font(.monoUI(13))
                            .foregroundStyle(palette.ink3Color)
                        chevron
                    }
                }
                .buttonStyle(PressableStyle())

                rowShell {
                    icon("internaldrive")
                    rowTitle("Storage used")
                    Spacer(minLength: 0)
                    Text(Format.bytes(downloads.totalBytesOnDisk()))
                        .font(.monoUI(13))
                        .foregroundStyle(palette.ink3Color)
                }

                if connectivity.pendingWrites > 0 {
                    rowShell {
                        icon("arrow.triangle.2.circlepath")
                        rowTitle("Waiting to sync")
                        Spacer(minLength: 0)
                        Text("\(connectivity.pendingWrites)")
                            .font(.monoUI(13))
                            .foregroundStyle(palette.warnColor)
                        Button("Sync now") {
                            Task {
                                await SyncEngine.shared.drain()
                                await connectivity.refreshPendingCount()
                            }
                        }
                        .font(.ui(13, weight: .medium))
                        .foregroundStyle(palette.accentColor)
                        .disabled(!connectivity.isOnline)
                    }
                }

                if connectivity.rejectedWrites > 0 {
                    // Tappable, not just a tally. A refused write is a change
                    // the reader made that the server never took, and once it
                    // stops holding off revalidation the server's answer
                    // overwrites it — so a bare number named a problem the
                    // reader had no way to look at or act on.
                    Button {
                        Haptics.tap()
                        showRejected = true
                    } label: {
                        rowShell {
                            icon("exclamationmark.triangle", tint: palette.badColor)
                            rowTitle("Couldn't sync")
                            Spacer(minLength: 0)
                            Text("\(connectivity.rejectedWrites)")
                                .font(.monoUI(13))
                                .foregroundStyle(palette.badColor)
                            chevron
                        }
                    }
                    .buttonStyle(PressableStyle())
                }

                actionRow("Clear cached covers", icon: "trash", tint: palette.ink1Color) {
                    Task {
                        await ImageCache.shared.clearDisk()
                        Haptics.success()
                    }
                }

                RecordRule()
            }
        }
    }

    private var sendToKindle: some View {
        group("Send to Kindle") {
            VStack(alignment: .leading, spacing: Spacing.md) {
                Plate {
                    PlateField(
                        label: "Kindle email",
                        text: $kindleEmail,
                        isFirst: true,
                        hint: "Optional",
                        keyboard: .emailAddress,
                        identifier: "kindle-email"
                    )
                }

                HStack(spacing: Spacing.md) {
                    Text(kindleError ?? "Where “Send to Kindle” delivers a book.")
                        .font(.ui(12))
                        .foregroundStyle(kindleError == nil ? palette.ink3Color : palette.badColor)

                    Spacer(minLength: 0)

                    Button(kindleSaved ? "Saved" : "Save") {
                        Task {
                            do {
                                try await AuthService.setKindleEmail(kindleEmail.nilIfBlank)
                                kindleError = nil
                                kindleSaved = true
                                Haptics.success()
                            } catch {
                                // Account configuration, so this is one of the
                                // writes the outbox will not take (rule 08) —
                                // which makes the failure the whole story.
                                // Swallowing it and flipping to "Saved" claimed
                                // a change that existed nowhere: not on the
                                // server, not in the queue, gone at the next
                                // launch.
                                kindleError = (error as? APIError)?.errorDescription
                                    ?? error.localizedDescription
                                Haptics.warning()
                            }
                        }
                    }
                    .font(.ui(13.5, weight: .semibold))
                    .foregroundStyle(kindleSaved ? palette.okColor : palette.accentColor)
                    .disabled(kindleSaved)
                    .contentTransition(.numericText())
                    .animation(Motion.snap, value: kindleSaved)
                }
            }
        }
    }

    /// The formats hidden from this reader's library All Books view — the
    /// tokens the scanners index, plus any saved token the list doesn't know
    /// (so it can always be un-hidden). Account configuration: saved directly,
    /// never queued (rule 08), disabled while offline.
    private var hiddenFormatsSection: some View {
        let options = KnownLibraryFormats.union(hiddenFormats).sorted()
        return group("Hidden formats") {
            VStack(alignment: .leading, spacing: Spacing.md) {
                FlowLayout(spacing: 6, lineSpacing: 6) {
                    ForEach(options, id: \.self) { format in
                        Button {
                            if hiddenFormats.contains(format) {
                                hiddenFormats.remove(format)
                            } else {
                                hiddenFormats.insert(format)
                            }
                        } label: {
                            Chip(label: format, isOn: hiddenFormats.contains(format))
                        }
                        .buttonStyle(PressableStyle())
                    }
                }

                HStack(spacing: Spacing.md) {
                    Text(hiddenError ?? "Hidden formats disappear from your All Books view; a book stays while any of its formats is visible.")
                        .font(.ui(12))
                        .foregroundStyle(hiddenError == nil ? palette.ink3Color : palette.badColor)

                    Spacer(minLength: 0)

                    Button(hiddenSaved ? "Saved" : "Save") {
                        Task {
                            do {
                                try await AuthService.setHiddenFormats(hiddenFormats.sorted())
                                // Repaint the identity so the library tab's
                                // `onChange(of: app.user?.hiddenFormats)`
                                // reshapes the grid immediately.
                                await app.refreshUser()
                                hiddenError = nil
                                hiddenSaved = true
                                Haptics.success()
                            } catch {
                                // Same rule-08 posture as the Kindle save
                                // above: no outbox, so the failure is the
                                // whole story — surface it, never "Saved".
                                hiddenError = (error as? APIError)?.errorDescription
                                    ?? error.localizedDescription
                                Haptics.warning()
                            }
                        }
                    }
                    .font(.ui(13.5, weight: .semibold))
                    .foregroundStyle(hiddenSaved ? palette.okColor : palette.accentColor)
                    .disabled(hiddenSaved || !connectivity.isOnline)
                    .contentTransition(.numericText())
                    .animation(Motion.snap, value: hiddenSaved)
                }
            }
        }
    }

    private var session: some View {
        VStack(spacing: 0) {
            actionRow("Change server", icon: "server.rack", tint: palette.ink1Color, isFirst: true) {
                Task { await app.changeServer() }
            }
            actionRow("Sign out", icon: "rectangle.portrait.and.arrow.right", tint: palette.badColor) {
                showSignOutConfirm = true
            }
            RecordRule()
        }
        .screenPadding()
    }

    private var colophon: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("Omnibus \(app.appVersion)")
            if let server = app.serverVersion {
                Text("Server \(server)")
            }
            if let url = app.serverURL {
                Text(url)
            }
        }
        .font(.monoUI(10.5))
        .foregroundStyle(palette.ink3Color.opacity(0.8))
        .screenPadding()
    }

    // MARK: - Row vocabulary

    private func group<Content: View>(
        _ title: String, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            SectionLabel(title)
            content()
        }
        .screenPadding()
    }

    private func linkRow(_ label: String, icon glyph: String, to destination: Destination) -> some View {
        NavigationLink(value: destination) {
            rowShell {
                icon(glyph)
                rowTitle(label)
                Spacer(minLength: 0)
                chevron
            }
        }
        .buttonStyle(PressableStyle())
    }

    private func actionRow(
        _ label: String, icon glyph: String, tint: Color? = nil,
        isFirst: Bool = false, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            rowShell(isFirst: isFirst) {
                icon(glyph, tint: tint)
                rowTitle(label, tint: tint)
                Spacer(minLength: 0)
            }
        }
        .buttonStyle(PressableStyle())
    }

    private func rowShell<Content: View>(
        isFirst: Bool = false, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(spacing: 0) {
            if !isFirst { Hairline() }
            HStack(spacing: Spacing.md) {
                content()
            }
            .padding(.vertical, 13)
            .contentShape(Rectangle())
        }
    }

    private func icon(_ name: String, tint: Color? = nil) -> some View {
        Image(systemName: name)
            .font(.system(size: 15, weight: .medium))
            .foregroundStyle(tint ?? palette.accentColor)
            .frame(width: 24, alignment: .leading)
    }

    private func rowTitle(_ text: String, tint: Color? = nil) -> some View {
        Text(text)
            .font(.ui(15.5))
            .foregroundStyle(tint ?? palette.ink0Color)
    }

    private var chevron: some View {
        Image(systemName: "chevron.right")
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(palette.ink3Color.opacity(0.7))
    }
}

/// The writes the server refused, and what it said about each.
///
/// These are changes the reader made on this device that the server never took.
/// They no longer hold off revalidation, so the local copy has already reverted
/// to the server's version — which makes naming them the only thing left that
/// can explain why something the reader did is no longer there.
private struct RejectedWritesSheet: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var ops: [PendingOp] = []

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.md) {
                    Text(
                        """
                        These changes were made on this device but the server \
                        wouldn't accept them, so they've been undone here. \
                        Discarding clears the list.
                        """
                    )
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink2Color)

                    ForEach(ops) { op in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(Self.label(for: op.kind))
                                .font(.ui(15, weight: .medium))
                                .foregroundStyle(palette.ink0Color)
                            Text(op.lastError ?? "Rejected")
                                .font(.ui(12.5))
                                .foregroundStyle(palette.badColor)
                            Text(
                                Date(timeIntervalSince1970: TimeInterval(op.createdAt))
                                    .formatted(.relative(presentation: .named))
                            )
                            .font(.ui(11.5))
                            .foregroundStyle(palette.ink3Color)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(Spacing.md)
                        .background(
                            RoundedRectangle(cornerRadius: 12)
                                .fill(palette.bg2Color)
                        )
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
            }
            .background(ScreenBackground())
            .navigationTitle("Couldn't sync")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Discard", role: .destructive) {
                        Task {
                            await Connectivity.shared.discardRejected()
                            Haptics.warning()
                            dismiss()
                        }
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task { ops = await OfflineStore.shared.rejectedOps() }
    }

    /// The op kind, said the way a reader would describe what they did.
    private static func label(for kind: String) -> String {
        if kind.hasPrefix("progress:") { return "Reading position" }
        if kind.hasPrefix("rating:") { return "Star rating" }
        if kind.hasPrefix("read_status:") { return "Read status" }
        if kind.hasPrefix("playback_rate:") { return "Playback speed" }
        switch kind {
        case OpKind.highlight: return "Highlight"
        case OpKind.bookmark: return "Bookmark"
        case OpKind.journal: return "Journal entry"
        case OpKind.session: return "Reading time"
        case OpKind.shelfMembership: return "Shelf change"
        default: return "Change"
        }
    }
}

/// One theme, shown in its own colours.
private struct ThemeSwatch: View {
    let name: ThemeName
    let isOn: Bool
    let action: () -> Void

    @Environment(\.palette) private var palette

    private var swatch: Palette { Palette.named(name) }

    var body: some View {
        Button(action: action) {
            VStack(spacing: 7) {
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(swatch.bg0Color)
                    .aspectRatio(1, contentMode: .fit)
                    .overlay {
                        VStack(spacing: 4) {
                            Capsule()
                                .fill(swatch.ink0Color)
                                .frame(width: 24, height: 3)
                            Capsule()
                                .fill(swatch.ink2Color)
                                .frame(width: 17, height: 3)
                            Capsule()
                                .fill(swatch.accentColor)
                                .frame(width: 11, height: 3)
                        }
                    }
                    // A dark swatch on a dark page needs a real edge to read as
                    // an object — at `line2` the Pure Black tile had none.
                    .overlay(
                        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                            .strokeBorder(
                                isOn ? palette.accentColor : palette.lineColor,
                                lineWidth: isOn ? 2 : 0.75
                            )
                    )

                Text(name.label)
                    .font(.ui(11, weight: isOn ? .semibold : .regular))
                    .foregroundStyle(isOn ? palette.ink0Color : palette.ink3Color)
                    .lineLimit(1)
            }
        }
        .buttonStyle(PressableStyle())
        .accessibilityLabel(name.label)
        .accessibilityAddTraits(isOn ? [.isSelected, .isButton] : .isButton)
    }
}

struct DownloadsView: View {
    @Environment(\.palette) private var palette
    private var downloads = DownloadManager.shared

    @State private var books: [String: Book] = [:]

    private var records: [DownloadRecord] {
        downloads.records.values.sorted { $0.updatedAt > $1.updatedAt }
    }

    var body: some View {
        Group {
            if records.isEmpty {
                EmptyStateView(
                    icon: "arrow.down.circle",
                    title: "Nothing downloaded",
                    message: "Download a book from its detail screen to read or listen offline."
                )
            } else {
                // Record rows on the page ground rather than a `List`: a
                // two-row list drew a filled slab that stopped halfway down an
                // otherwise black screen, and its rows led nowhere.
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(records.enumerated()), id: \.element.id) { index, record in
                            NavigationLink(value: Destination.book(uuid: record.bookUUID)) {
                                row(record, isFirst: index == 0)
                            }
                            .buttonStyle(PressableStyle())
                            .contextMenu {
                                Button(role: .destructive) {
                                    Task { await downloads.remove(record.bookUUID, kind: record.kind) }
                                } label: {
                                    Label("Remove download", systemImage: "trash")
                                }
                            }
                        }
                        RecordRule()

                        Text("\(Format.bytes(downloads.totalBytesOnDisk())) on this device")
                            .font(.monoUI(11))
                            .foregroundStyle(palette.ink3Color)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.top, Spacing.md)
                    }
                    .screenPadding()
                    .padding(.top, Spacing.sm)
                    .padding(.bottom, 40)
                }
                .scrollIndicators(.hidden)
            }
        }
        .background(ScreenBackground())
        .navigationTitle("Downloads")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            // Titles come from the replica so this screen works offline too —
            // and from the library mirror when the replica has nothing, which
            // is the case for every book downloaded straight from the grid
            // without ever opening its detail screen. Those rows used to render
            // untitled.
            for record in records where books[record.bookUUID] == nil {
                if let book: Book = await Cache.cachedOnly(CacheKey.book(record.bookUUID)) {
                    books[record.bookUUID] = book
                } else if let book = await LibraryIndex.shared.book(uuid: record.bookUUID) {
                    books[record.bookUUID] = book
                }
            }
            await refreshStaleness()
        }
    }

    /// Re-read each completed download's detail metadata so the rows can say
    /// whether the library file has moved under them.
    ///
    /// It has to be the *detail* read: the library listing projection carries
    /// no per-file rows, so a replaced file is invisible in the mirror this
    /// screen otherwise reads from. Bounded by the number of downloads, which
    /// is small by nature, and skipped entirely offline — where the answer
    /// could not be acted on anyway.
    private func refreshStaleness() async {
        guard Connectivity.shared.isOnline else { return }
        for record in records where record.state == .complete {
            do {
                // Only the fresh (server-confirmed) read can settle this —
                // the replica's provisional answer is what we already have.
                for try await read in LibraryService.book(uuid: record.bookUUID)
                where read.isFresh {
                    books[record.bookUUID] = read.value
                }
            } catch {
                // A row that can't refresh keeps showing what it had.
                continue
            }
        }
    }

    private func row(_ record: DownloadRecord, isFirst: Bool) -> some View {
        let book = books[record.bookUUID]

        return VStack(spacing: 0) {
            if !isFirst { Hairline() }

            HStack(spacing: Spacing.md) {
                BookCover(
                    identity: CoverIdentity(
                        uuid: record.bookUUID,
                        title: book?.displayTitle ?? "?",
                        author: book?.creators.first?.name,
                        accent: book?.accent,
                        hasCover: book?.coverURL != nil
                    ),
                    size: .sm
                )
                .frame(width: 38)
                .coverShadow(0.6)

                VStack(alignment: .leading, spacing: 3) {
                    Text(book?.displayTitle ?? record.bookUUID)
                        .font(.ui(14.5, weight: .medium))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(1)

                    HStack(spacing: 6) {
                        Badge(text: record.format)
                        Text(statusLabel(record, isStale: isStale(record)))
                            .font(.ui(11.5))
                            .foregroundStyle(statusTint(record, isStale: isStale(record)))
                    }

                    if record.state == .running {
                        ProgressBar(fraction: record.fraction, tint: palette.accentColor)
                            .frame(width: 140)
                    }
                }

                Spacer(minLength: 0)

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.ink3Color.opacity(0.7))
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
    }

    /// Whether this row's library file has moved under the downloaded copy.
    /// `false` until a refresh has produced detail metadata to compare
    /// against — the listing projection carries no per-file validators.
    private func isStale(_ record: DownloadRecord) -> Bool {
        guard let book = books[record.bookUUID] else { return false }
        return downloads.isStale(record.bookUUID, kind: record.kind, against: book)
    }

    private func statusLabel(_ record: DownloadRecord, isStale: Bool) -> String {
        switch record.state {
        case .complete:
            isStale
                ? "Update available · \(Format.bytes(record.totalBytes))"
                : Format.bytes(record.totalBytes)
        case .running: "\(Int(record.fraction * 100))%"
        case .queued: "Queued"
        case .failed: record.error ?? "Failed"
        }
    }

    /// `warn`, not `bad`, for a superseded copy: the file on the device still
    /// opens and still reads — it is just no longer the newest one, which is
    /// an invitation rather than a failure.
    private func statusTint(_ record: DownloadRecord, isStale: Bool) -> Color {
        if record.state == .failed { return palette.badColor }
        return isStale ? palette.warnColor : palette.ink3Color
    }
}
