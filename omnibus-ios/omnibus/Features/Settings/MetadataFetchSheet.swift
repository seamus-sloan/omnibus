//  MetadataFetchSheet.swift
//  "Fetch metadata": find this book at every configured provider, then move
//  the fields you pick into the editor's draft.
//
//  Three phases in one sheet — ask, choose, compare — rather than the web
//  picker's two-column table in a scrim, which needs a desktop's width to be
//  readable at all. Each phase fills the screen, states what it is waiting for,
//  and keeps its one primary action pinned above the home indicator where a
//  thumb can reach it.
//
//  Nothing here saves. Taking a field writes into the editor's `draft`, so the
//  editor's Save button stays the single writer and its dirty tracking,
//  validation, and changed-fields-only payload keep working untouched. The
//  cover is the one exception — it cannot stage — and says so on its own card.

import SwiftUI

struct MetadataFetchSheet: View {
    let uuid: String
    /// The book's current cover, for the compare screen's yours-vs-theirs.
    let identity: CoverIdentity
    /// Written into as fields are taken. The editor owns it.
    @Binding var draft: MetadataDraft
    /// The book as loaded — the baseline "is this field carrying a change?"
    /// is measured against, and where Undo puts a field back to.
    let loaded: MetadataDraft
    /// Runs as the sheet closes, with a line about what it staged (or `nil`).
    var onClose: (String?) -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var stage: MetadataFetchStage = .ready
    @State private var queryTitle: String
    @State private var queryAuthor: String
    @State private var queryISBN: String

    @State private var providers: [ProviderInfo] = []
    /// Provider ids the reader has switched *off* for this search. Held as the
    /// negative so a provider that appears later (a key added mid-session) is
    /// asked by default rather than silently excluded.
    @State private var muted: Set<String> = []

    @State private var editions: [ProviderEdition] = []
    @State private var sources: [ProviderSearchSource] = []
    /// A selected candidate is re-fetched in full behind the reveal; until it
    /// lands the compare screen is showing the thinner search hit, so nothing
    /// on it may be taken.
    @State private var isHydrating = false
    @State private var showAllFields = false

    /// Which fields *this sheet* took, so the closing note counts what it did
    /// rather than every edit the form is carrying.
    @State private var taken: Set<MetadataFetchField> = []
    @State private var takenFrom: MetadataProvider?

    @State private var coverStatus: String?
    @State private var isApplyingCover = false
    /// Bumped after a cover applies so the thumbnail is re-read rather than
    /// served from the image cache it is already sitting in.
    @State private var coverRevision = 0

    init(
        uuid: String,
        identity: CoverIdentity,
        draft: Binding<MetadataDraft>,
        loaded: MetadataDraft,
        onClose: @escaping (String?) -> Void
    ) {
        self.uuid = uuid
        self.identity = identity
        _draft = draft
        self.loaded = loaded
        self.onClose = onClose
        // Seeded from the draft, including its ISBN, because that is what the
        // book says. It does narrow the search hard — every provider goes to
        // its exact-identifier lookup — but that is the honest answer to the
        // question the fields are asking, and the reader can see it sitting
        // there and clear it before searching.
        let current = draft.wrappedValue
        _queryTitle = State(initialValue: current.title.nilIfBlank ?? "")
        _queryAuthor = State(initialValue: current.authors.first?.nilIfBlank ?? "")
        _queryISBN = State(initialValue: current.isbn13.nilIfBlank ?? "")
    }

    var body: some View {
        NavigationStack {
            phase
                .background(ScreenBackground())
                .navigationTitle(navigationTitle)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { toolbar }
                .safeAreaInset(edge: .bottom) { bottomBar }
                .task { await loadProviders() }
        }
    }

    // MARK: - Phases

    @ViewBuilder
    private var phase: some View {
        switch stage {
        case .ready:
            askScreen
        case .searching:
            searchingScreen
        case .results:
            resultsScreen
        case let .failed(message):
            ErrorStateView(message: message) { Task { await search() } }
                .frame(maxHeight: .infinity, alignment: .center)
        case let .compare(edition):
            CompareScreen(
                edition: edition,
                draft: $draft,
                loaded: loaded,
                identity: identity,
                isHydrating: isHydrating,
                showAllFields: showAllFields,
                coverStatus: coverStatus,
                isApplyingCover: isApplyingCover,
                coverRevision: coverRevision,
                onTake: { take($0, from: edition) },
                onUndo: { undo($0) },
                onApplyCover: { Task { await applyCover(edition) } }
            )
        }
    }

    private var navigationTitle: String {
        switch stage {
        case .ready: "Fetch metadata"
        case .searching: "Searching"
        case .results: editions.isEmpty ? "No matches" : "\(editions.count) editions"
        case .failed: "Search failed"
        case .compare: "What would change"
        }
    }

    // MARK: - Phase one: ask

    private var askScreen: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Find this edition")
                        .font(.display(27))
                        .foregroundStyle(palette.ink0Color)
                    Text(
                        "Every source you've set up gets asked at once. Nothing is written until you take a field and press Save."
                    )
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink2Color)
                    .fixedSize(horizontal: false, vertical: true)
                }

                Plate {
                    PlateField(label: "Title", text: $queryTitle, isFirst: true)
                    PlateField(label: "Author", text: $queryAuthor)
                    PlateField(
                        label: "ISBN",
                        text: $queryISBN,
                        hint: "one exact edition",
                        keyboard: .numbersAndPunctuation
                    )
                }

                if queryISBN.nilIfBlank != nil { isbnNote }

                sourcePicker
            }
            .screenPadding()
            .padding(.top, Spacing.md)
            .padding(.bottom, Spacing.lg)
        }
        .scrollIndicators(.hidden)
    }

    /// The ISBN changes the *shape* of the answer, not just its ranking, so
    /// the screen says so where the field is rather than leaving the reader to
    /// discover it from a one-row result list.
    private var isbnNote: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: "info.circle")
                .font(.system(size: 12))
                .foregroundStyle(palette.ink3Color)
                .padding(.top, 1)
            Text("An ISBN narrows every source to that one printing. Clear it to browse other editions.")
                .font(.ui(12.5))
                .foregroundStyle(palette.ink2Color)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: Spacing.sm)
            Button("Clear") {
                Haptics.tap()
                withAnimation(Motion.snap) { queryISBN = "" }
            }
            .font(.ui(12.5, weight: .semibold))
            .foregroundStyle(palette.accentColor)
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 2)
    }

    /// Which sources to ask. The wire has always taken a provider list; this
    /// is what plugs into it — and it is what lets a reader whose Google Books
    /// quota is spent ask the other two without waiting on it.
    @ViewBuilder
    private var sourcePicker: some View {
        if !providers.isEmpty {
            VStack(alignment: .leading, spacing: Spacing.sm) {
                SectionLabel("Ask")
                FlowLayout(spacing: 7, lineSpacing: 7) {
                    ForEach(providers) { provider in
                        Button {
                            toggle(provider)
                        } label: {
                            Chip(
                                label: provider.displayName,
                                isOn: isAsked(provider),
                                systemImage: provider.configured
                                    ? (isAsked(provider) ? "checkmark" : nil)
                                    : "key.slash"
                            )
                        }
                        .buttonStyle(.plain)
                        .disabled(!provider.configured)
                        .opacity(provider.configured ? 1 : 0.45)
                        .accessibilityLabel(
                            provider.configured
                                ? "\(provider.displayName), \(isAsked(provider) ? "asked" : "not asked")"
                                : "\(provider.displayName), no API key"
                        )
                    }
                }
                if providers.contains(where: { !$0.configured }) {
                    Text("Greyed-out sources need an API key on the server.")
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                }
            }
        }
    }

    // MARK: - Phase two: choose

    private var searchingScreen: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.md) {
                Text("Asking \(askedNames)\u{2026}")
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink2Color)
                    .accessibilityAddTraits(.updatesFrequently)
                MetadataFetchSkeleton(rows: 4)
            }
            .screenPadding()
            .padding(.vertical, Spacing.md)
        }
        .scrollIndicators(.hidden)
    }

    private var resultsScreen: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.sm) {
                if editions.isEmpty {
                    EmptyStateView(
                        icon: "magnifyingglass",
                        title: "Nothing matched",
                        message:
                            "Try a shorter title, drop the author, or clear the ISBN — an ISBN only ever finds one printing."
                    )
                } else {
                    ForEach(Array(editions.enumerated()), id: \.element.id) { index, edition in
                        EditionCandidateCard(
                            edition: edition,
                            changes: MetadataFetchFlow.changeCount(edition: edition, draft: draft)
                        ) {
                            Task { await select(edition) }
                        }
                        .cascadeIn(index: index)
                    }
                }

                if !sources.isEmpty {
                    ProviderSourceStrip(sources: sources)
                        .padding(.top, Spacing.md)
                }
            }
            .screenPadding()
            .padding(.top, Spacing.sm)
            .padding(.bottom, Spacing.lg)
        }
        .scrollIndicators(.hidden)
    }

    // MARK: - Chrome

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            if let target = backTarget {
                Button {
                    Haptics.tap()
                    withAnimation(Motion.settle) { stage = target }
                } label: {
                    Image(systemName: "chevron.left")
                }
                .accessibilityLabel("Back")
            }
        }
        ToolbarItem(placement: .confirmationAction) {
            Button("Done") { close() }
        }
    }

    /// Where the back chevron goes, or `nil` on the first screen — which has
    /// no back, only Done.
    private var backTarget: MetadataFetchStage? {
        switch stage {
        case .ready, .searching: nil
        case .results, .failed: .ready
        // Back to the list, not to the form: the reader is choosing between
        // candidates, and losing the results to see another one is what makes
        // a picker feel like a wizard.
        case .compare: .results
        }
    }

    @ViewBuilder
    private var bottomBar: some View {
        VStack(spacing: Spacing.sm) {
            switch stage {
            case .ready, .failed:
                Button {
                    Task { await search() }
                } label: {
                    Text("Search")
                }
                .buttonStyle(FilledButtonStyle())
                .disabled(!canSearch)
                .opacity(canSearch ? 1 : 0.5)

            case .searching:
                Button {} label: {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small).tint(palette.accentInkColor)
                        Text("Searching\u{2026}")
                    }
                }
                .buttonStyle(FilledButtonStyle())
                .disabled(true)

            case .results:
                Button {
                    Haptics.tap()
                    withAnimation(Motion.settle) { stage = .ready }
                } label: {
                    Text("Refine search").frame(maxWidth: .infinity)
                }
                .buttonStyle(QuietButtonStyle())

            case let .compare(edition):
                compareBar(edition)
            }
        }
        .screenPadding()
        .padding(.top, Spacing.sm)
        .padding(.bottom, Spacing.sm)
        .background(.bar)
    }

    @ViewBuilder
    private func compareBar(_ edition: ProviderEdition) -> some View {
        let changes = MetadataFetchFlow.changeCount(edition: edition, draft: draft)
        Button {
            takeAll(edition)
        } label: {
            Text(MetadataFetchFlow.takeAllLabel(changes: changes) ?? "Nothing left to take")
        }
        .buttonStyle(FilledButtonStyle())
        .disabled(isHydrating || changes == 0)
        .opacity(isHydrating || changes == 0 ? 0.5 : 1)

        HStack(spacing: Spacing.md) {
            Button {
                Haptics.select()
                withAnimation(Motion.settle) { showAllFields.toggle() }
            } label: {
                Label(
                    showAllFields ? "Only differences" : "Show every field",
                    systemImage: showAllFields ? "line.3.horizontal.decrease" : "list.bullet"
                )
                .font(.ui(13, weight: .medium))
                .foregroundStyle(palette.accentColor)
            }
            .buttonStyle(.plain)

            Spacer(minLength: 0)

            Text("Not saved yet")
                .font(.monoUI(10))
                .foregroundStyle(palette.ink3Color)
        }
    }

    // MARK: - Derived

    private var askedProviders: [MetadataProvider]? {
        let asked = providers.filter { $0.configured && !muted.contains($0.id.rawValue) }
        // `nil` means "every configured one", which is also what an untouched
        // picker means — and what a client whose catalog read failed must send,
        // since an empty list is a 400.
        guard !asked.isEmpty, asked.count != providers.filter(\.configured).count else {
            return nil
        }
        return asked.map(\.id)
    }

    private var askedNames: String {
        let asked = providers.filter { $0.configured && !muted.contains($0.id.rawValue) }
        guard !asked.isEmpty else { return "every configured source" }
        return asked.map(\.displayName).joined(separator: ", ")
    }

    private var canSearch: Bool {
        MetadataFetchFlow.searchRequest(
            title: queryTitle, author: queryAuthor, isbn: queryISBN, providers: askedProviders
        ) != nil
    }

    private func isAsked(_ provider: ProviderInfo) -> Bool {
        provider.configured && !muted.contains(provider.id.rawValue)
    }

    private func toggle(_ provider: ProviderInfo) {
        guard provider.configured else { return }
        Haptics.select()
        withAnimation(Motion.snap) {
            if muted.contains(provider.id.rawValue) {
                muted.remove(provider.id.rawValue)
            } else {
                // Never leave nothing selected: an empty provider list is a
                // 400, and a Search button that 400s is worse than one that
                // refuses the last un-tick.
                let asked = providers.filter { isAsked($0) }
                guard asked.count > 1 else { return }
                muted.insert(provider.id.rawValue)
            }
        }
    }

    // MARK: - Staging

    private func take(_ field: MetadataFetchField, from edition: ProviderEdition) {
        withAnimation(Motion.snap) {
            field.apply(to: &draft, from: edition)
            taken.insert(field)
        }
        takenFrom = edition.source
    }

    private func undo(_ field: MetadataFetchField) {
        withAnimation(Motion.snap) {
            field.undo(in: &draft, to: loaded)
            taken.remove(field)
        }
    }

    private func takeAll(_ edition: ProviderEdition) {
        Haptics.success()
        withAnimation(Motion.settle) {
            for field in MetadataFetchField.allCases where field.isAvailable(edition) {
                guard field.differs(draft: draft, edition: edition) else { continue }
                field.apply(to: &draft, from: edition)
                taken.insert(field)
            }
        }
        takenFrom = edition.source
    }

    private func close() {
        let note = takenFrom.flatMap {
            MetadataFetchFlow.stagedNote(count: taken.count, source: $0)
        }
        onClose(note)
        dismiss()
    }

    // MARK: - Network

    private func loadProviders() async {
        guard providers.isEmpty else { return }
        // Best-effort: the editor's own gate already established that at least
        // one source is configured, so a failed catalog read costs the source
        // picker and nothing else — the search still asks every provider.
        providers = (try? await APIClient.shared.get("/api/metadata/providers")) ?? []
    }

    private func search() async {
        guard
            let request = MetadataFetchFlow.searchRequest(
                title: queryTitle, author: queryAuthor, isbn: queryISBN, providers: askedProviders
            )
        else { return }
        withAnimation(Motion.settle) { stage = .searching }
        do {
            let response: EditionSearchResponse = try await APIClient.shared.post(
                "/api/metadata/editions/search", body: request
            )
            editions = MetadataFetchFlow.ordered(response.editions, query: request.query)
            sources = response.sources
            Haptics.tap()
            withAnimation(Motion.settle) { stage = .results }
        } catch {
            withAnimation(Motion.settle) { stage = .failed(message(error)) }
        }
    }

    /// Open the compare screen for one candidate, and re-fetch it in full
    /// behind the reveal.
    ///
    /// The merge is one-directional on purpose: the detail record fills in what
    /// the list row lacked and can never take a field away from it, so the row
    /// the reader tapped is still the row they get.
    private func select(_ edition: ProviderEdition) async {
        Haptics.tap()
        coverStatus = nil
        showAllFields = false
        isHydrating = true
        withAnimation(Motion.settle) { stage = .compare(edition) }

        let fetched: ProviderEdition? = try? await APIClient.shared.post(
            "/api/metadata/editions/hydrate",
            body: EditionHydrateRequest(
                source: edition.source,
                providerRef: edition.providerRef,
                isbn13: edition.isbn13
            )
        )
        let showingOurs = MetadataFetchFlow.hydrateShouldApply(stage: stage, asked: edition)
        if showingOurs, let fetched {
            withAnimation(Motion.settle) {
                stage = .compare(MetadataFetchFlow.merged(fetched: fetched, thinner: edition))
            }
        }
        // Left alone only while a *newer* selection is in flight — that
        // request owns the flag and will clear it when it lands.
        if showingOurs || !isComparing {
            isHydrating = false
        }
    }

    private var isComparing: Bool {
        if case .compare = stage { return true }
        return false
    }

    /// The one write this sheet makes. The device can't hand the server a
    /// provider's image (it would have to fetch it cross-origin and re-upload
    /// it), so the server fetches it — which is why this cannot be staged with
    /// the fields and why the card says so.
    private func applyCover(_ edition: ProviderEdition) async {
        guard let url = MetadataFetchFlow.coverURL(edition), !isApplyingCover else { return }
        isApplyingCover = true
        coverStatus = "Applying cover\u{2026}"
        defer { isApplyingCover = false }
        do {
            let _: Book = try await APIClient.shared.post(
                "/api/ebooks/\(uuid)/cover/from-url", body: CoverFromURLRequest(url: url)
            )
            // Every thumb size is regenerated server-side, so every cached one
            // is stale — including the sizes this screen isn't showing, which
            // the grid behind it is.
            for size in [ThumbSize.sm, .md, .lg] {
                await ImageCache.shared.invalidate("/api/thumbs/\(uuid)/\(size.rawValue)")
            }
            await OfflineStore.shared.cacheDelete(CacheKey.book(uuid))
            Haptics.success()
            coverRevision += 1
            coverStatus = "Cover updated \u{b7} saved already"
        } catch {
            coverStatus = message(error)
        }
    }

    private func message(_ error: Error) -> String {
        (error as? APIError)?.errorDescription ?? error.localizedDescription
    }
}

// MARK: - Phase three: compare

/// The selected edition beside the book, as one card per field.
///
/// Split out so its `showAll` filtering and the field list it renders sit
/// together — and so the sheet above stays the one place that talks to the
/// network.
private struct CompareScreen: View {
    let edition: ProviderEdition
    @Binding var draft: MetadataDraft
    let loaded: MetadataDraft
    let identity: CoverIdentity
    let isHydrating: Bool
    let showAllFields: Bool
    let coverStatus: String?
    let isApplyingCover: Bool
    let coverRevision: Int
    let onTake: (MetadataFetchField) -> Void
    let onUndo: (MetadataFetchField) -> Void
    let onApplyCover: () -> Void

    @Environment(\.palette) private var palette

    /// A staged field stays on screen even once it stops differing: taking a
    /// field makes the two sides agree, and a card that vanished the instant
    /// you pressed it would take the evidence of what you did with it.
    private var shown: [MetadataFetchField] {
        MetadataFetchField.allCases.filter { field in
            showAllFields
                || field.differs(draft: draft, edition: edition)
                || field.isStaged(draft: draft, loaded: loaded)
        }
    }

    private var sourceCoverURL: String? { MetadataFetchFlow.coverURL(edition) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.md) {
                header

                // Nothing real is shown until the record has settled. The
                // search hit this screen opens with is a partial answer — it
                // carries fewer fields than the provider's own record — so
                // rendering it means cards appearing and rearranging under the
                // reader a moment later. A placeholder for that moment is
                // calmer than a wrong answer corrected in view.
                if isHydrating {
                    Text("Loading the full record\u{2026}")
                        .font(.ui(13))
                        .foregroundStyle(palette.ink2Color)
                        .accessibilityAddTraits(.updatesFrequently)
                    MetadataFetchSkeleton(rows: 3, showsCover: false)
                } else {
                    if sourceCoverURL != nil || showAllFields {
                        EditionCoverCard(
                            identity: identity,
                            sourceName: edition.source.displayName,
                            sourceURL: sourceCoverURL,
                            isBusy: isHydrating,
                            revision: coverRevision,
                            status: coverStatus,
                            isApplying: isApplyingCover,
                            onApply: onApplyCover
                        )
                    }

                    if shown.isEmpty {
                        EmptyStateView(
                            icon: "checkmark.seal",
                            title: "Already a match",
                            message:
                                "This edition agrees with your book on every field it knows about."
                        )
                    }

                    ForEach(shown) { field in
                        EditionFieldCard(
                            field: field,
                            sourceName: edition.source.displayName,
                            current: field.current(draft),
                            offered: field.sourceValue(edition),
                            original: field.original(loaded),
                            isStaged: field.isStaged(draft: draft, loaded: loaded),
                            isBusy: isHydrating,
                            onTake: { onTake(field) },
                            onUndo: { onUndo(field) }
                        )
                    }
                }
            }
            .screenPadding()
            .padding(.top, Spacing.sm)
            .padding(.bottom, Spacing.lg)
        }
        .scrollIndicators(.hidden)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            Group {
                if let sourceCoverURL {
                    ExternalImage(url: sourceCoverURL) { CoverPlate(title: edition.title) }
                        .aspectRatio(2.0 / 3.0, contentMode: .fit)
                        .clipShape(RoundedRectangle(cornerRadius: Radius.sm, style: .continuous))
                } else {
                    CoverPlate(title: edition.title)
                }
            }
            .frame(width: 52)

            VStack(alignment: .leading, spacing: 5) {
                Text(edition.title)
                    .font(.display(21))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(3)
                Text(MetadataFetchFlow.authorsLine(edition))
                    .font(.ui(13))
                    .foregroundStyle(palette.ink2Color)
                    .lineLimit(2)
                ProviderBadge(provider: edition.source)
                    .padding(.top, 2)
            }

            Spacer(minLength: 0)
        }
        .padding(.bottom, Spacing.xs)
    }
}
