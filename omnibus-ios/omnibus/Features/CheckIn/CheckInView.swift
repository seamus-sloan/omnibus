//  CheckInView.swift
//  Physical check-in: scan an ISBN barcode with VisionKit, resolve it against
//  the library, then check in / add / wishlist.
//
//  Replaces the ZXing WASM scanner the hybrid build ships — VisionKit's
//  `DataScannerViewController` is faster, handles focus and lighting itself,
//  and needs no 1.5 MB of WebAssembly.

import AVFoundation
import SwiftUI
import Vision
import VisionKit

struct CheckInView: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var manualISBN = ""
    @State private var stage: CheckInStage = .scan
    @State private var isResolving = false
    @State private var isWriting = false
    @State private var error: String?
    @State private var searchQuery = ""
    @State private var isSearching = false
    /// `nil` until a title search lands, so the no-matches line can't show
    /// before anyone has searched.
    @State private var searchResults: [ExternalBookMeta]?
    @State private var scannerAvailable = DataScannerViewController.isSupported
        && DataScannerViewController.isAvailable
    @State private var note = ""
    /// Book detail presented full-screen over the sheet, so dismissing it
    /// lands back on the scanner rather than losing the check-in loop.
    @State private var detailTarget: DetailTarget?

    private struct DetailTarget: Identifiable {
        let uuid: String
        var id: String { uuid }
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                switch stage {
                case .scan:
                    scannerSection
                case let .outcome(outcome):
                    outcomeSection(outcome)
                case let .success(success):
                    CheckInSuccessView(
                        success: success,
                        onScanAnother: { restart() },
                        onViewBook: { detailTarget = DetailTarget(uuid: $0) }
                    )
                }
            }
            .background(ScreenBackground())
            .navigationTitle("Check in")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        // Full-screen detail rather than a push inside the sheet; closing it
        // returns to the check-in flow, so the scan loop is never lost.
        .fullScreenCover(item: $detailTarget) { target in
            NavigationStack {
                BookDetailView(uuid: target.uuid)
                    .withDestinations()
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            Button {
                                detailTarget = nil
                            } label: {
                                Image(systemName: "chevron.left")
                            }
                        }
                    }
            }
        }
    }

    // MARK: - Scanning

    /// Scrolls so `SEARCH_LIMIT` (8) result rows under a 300pt camera view
    /// aren't clipped, and so the keyboard doesn't cover the fields.
    private var scannerSection: some View {
        ScrollView {
            VStack(spacing: Spacing.lg) {
                if scannerAvailable {
                    BarcodeScannerView { code in
                        guard !isResolving else { return }
                        Haptics.success()
                        Task { await resolve(code) }
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 300)
                    .clipShape(RoundedRectangle(cornerRadius: Radius.lg, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                            .strokeBorder(palette.lineColor, lineWidth: 1)
                    )
                    // VisionKit's guidance text ("Find nearby barcodes") isn't
                    // customizable, so guidance is disabled and drawn here instead.
                    .overlay(alignment: .top) {
                        Text("Move barcode into view")
                            .font(.ui(13, weight: .medium))
                            .foregroundStyle(.white)
                            .padding(.horizontal, Spacing.md)
                            .padding(.vertical, 6)
                            .background(Capsule().fill(.black.opacity(0.55)))
                            .padding(.top, Spacing.md)
                    }
                    .screenPadding()
                    .padding(.top, Spacing.lg)

                    Text("Point the camera at the book's barcode.")
                        .font(.ui(13))
                        .foregroundStyle(palette.ink2Color)
                } else {
                    EmptyStateView(
                        icon: "barcode.viewfinder",
                        title: "Camera unavailable",
                        message: "Enter the book's details by hand instead."
                    )
                }

                // Either field alone is enough, so both are offered up front rather
                // than making a title search something you reach only by failing an
                // ISBN first: plenty of copies — old paperbacks, book-club editions
                // — carry a barcode no ISBN index knows.
                VStack(alignment: .leading, spacing: Spacing.sm) {
                    Text("Or enter book details")
                        .font(.ui(13, weight: .medium))
                        .foregroundStyle(palette.ink2Color)
                    isbnField
                    titleField(placeholder: "Book Title (Optional)")
                    searchResultsList
                }
                .screenPadding()

                if let error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.ui(13))
                        .foregroundStyle(palette.badColor)
                        .screenPadding()
                }
            }
            .padding(.bottom, Spacing.lg)
        }
        .scrollIndicators(.hidden)
    }

    /// Names one edition, so it takes the precise `resolve` path rather than the
    /// provider title search.
    private var isbnField: some View {
        HStack {
            TextField("ISBN (13 digit) (Optional)", text: $manualISBN)
                .textFieldStyle(OmnibusFieldStyle())
                .keyboardType(.numbersAndPunctuation)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .onSubmit { Task { await resolve(manualISBN) } }
            Button {
                Task { await resolve(manualISBN) }
            } label: {
                if isResolving {
                    ProgressView()
                } else {
                    Image(systemName: "arrow.right.circle.fill").font(.system(size: 26))
                }
            }
            .disabled(manualISBN.count < 10 || isResolving)
            .foregroundStyle(palette.accentColor)
            .accessibilityLabel("Look up ISBN")
        }
    }

    // MARK: - Outcome

    @ViewBuilder
    private func outcomeSection(_ outcome: ScanOutcome) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                switch outcome {
                case let .alreadyOwned(book):
                    resultCard(
                        title: book.title, authors: book.authors,
                        cover: .library(uuid: book.uuid),
                        badge: "Already on your shelf", tint: palette.okColor
                    )
                    Text("You've already checked in a physical copy of this book.")
                        .font(.ui(14))
                        .foregroundStyle(palette.ink2Color)
                    detailsLink(uuid: book.uuid, style: .filled)

                case let .onWishlist(book):
                    resultCard(
                        title: book.title, authors: book.authors,
                        cover: .library(uuid: book.uuid),
                        badge: "On your wishlist", tint: palette.accentColor
                    )
                    Text("Checking a copy in clears this book from your wishlist.")
                        .font(.ui(14))
                        .foregroundStyle(palette.ink2Color)
                    checkInButton(book: book, isbn: book.isbn, label: "Check in this copy")
                    detailsLink(uuid: book.uuid, style: .quiet)

                case let .inLibraryUnowned(book):
                    resultCard(
                        title: book.title, authors: book.authors,
                        cover: .library(uuid: book.uuid),
                        badge: "In your library", tint: palette.accentColor
                    )
                    noteField
                    checkInButton(book: book, isbn: book.isbn, label: "Check in this copy")

                case let .closeMatch(book, scanned):
                    resultCard(
                        title: book.title, authors: book.authors,
                        cover: .library(uuid: book.uuid),
                        badge: "Possible match", tint: palette.warnColor
                    )
                    HStack(alignment: .top, spacing: Spacing.md) {
                        if let cover = scanned.coverURL, CheckInFlow.isExternalURL(cover) {
                            ExternalImage(url: cover) { Color.clear }
                                .aspectRatio(2.0 / 3.0, contentMode: .fit)
                                .frame(width: 40)
                                .clipShape(RoundedRectangle(cornerRadius: Radius.sm, style: .continuous))
                        }
                        VStack(alignment: .leading, spacing: 4) {
                            Text("You scanned")
                                .font(.ui(12, weight: .medium))
                                .foregroundStyle(palette.ink3Color)
                            Text(scanned.title)
                                .font(.ui(14, weight: .medium))
                                .foregroundStyle(palette.ink0Color)
                            Text(scanned.authorDisplay)
                                .font(.ui(12.5))
                                .foregroundStyle(palette.ink2Color)
                            ForEach(CheckInFlow.detailLines(for: scanned), id: \.self) { line in
                                Text(line)
                                    .font(.ui(11.5))
                                    .foregroundStyle(palette.ink3Color)
                            }
                        }
                    }
                    .padding(Spacing.md)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                            .fill(palette.bg1Color)
                    )
                    checkInButton(book: book, isbn: scanned.isbn13, label: "Yes, same book — check in")
                    Button("No, add as a new physical book") {
                        Task { await addPhysicalOnly(scanned) }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .frame(maxWidth: .infinity)
                    .disabled(isWriting)
                    .opacity(isWriting ? 0.55 : 1)

                case let .notInLibrary(online):
                    resultCard(
                        title: online.title, authors: online.authors,
                        cover: externalCover(online),
                        badge: "Not in your library", tint: palette.ink2Color,
                        details: CheckInFlow.detailLines(for: online)
                    )
                    Button("Add as physical book") {
                        Task { await addPhysicalOnly(online) }
                    }
                    .buttonStyle(FilledButtonStyle())
                    .disabled(isWriting)
                    .opacity(isWriting ? 0.55 : 1)
                    Button("Add to wishlist") {
                        Task { await addWishlist(online) }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .frame(maxWidth: .infinity)
                    .disabled(isWriting)
                    .opacity(isWriting ? 0.55 : 1)

                case .unresolved:
                    EmptyStateView(
                        icon: "questionmark.circle",
                        title: "Couldn't identify that book",
                        message: "Neither your library nor the online providers recognised that ISBN."
                    )
                    searchSection
                }

                // Without this the outcome screen's writes failed in silence —
                // only the scanner section rendered `error`, so a rejected
                // check-in / add / wishlist looked like a dead button.
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.ui(13))
                        .foregroundStyle(palette.badColor)
                }

                Button("Scan another") {
                    restart()
                }
                .font(.ui(14))
                .foregroundStyle(palette.accentColor)
                .frame(maxWidth: .infinity)
                .padding(.top, Spacing.sm)
                // Restarting mid-write would race the pending POST's stage
                // swap — a stale success screen for the previous book.
                .disabled(isWriting)
            }
            .screenPadding()
            .padding(.vertical, Spacing.lg)
        }
    }

    private var noteField: some View {
        TextField("Edition note (optional)", text: $note, axis: .vertical)
            .textFieldStyle(OmnibusFieldStyle())
            .lineLimit(1...3)
    }

    // MARK: - Title search (unresolved fallback)

    /// Some editions simply aren't in any ISBN index — typing the book's name
    /// is the way forward from an unresolved scan.
    private var searchSection: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text("Or search by title")
                .font(.ui(13, weight: .medium))
                .foregroundStyle(palette.ink2Color)
            titleField(placeholder: "The Name of the Wind")
            searchResultsList
        }
    }

    /// Shared with the scanner page so there is one `search` path, not two.
    /// Only the placeholder differs: beside an ISBN box the title is optional,
    /// on the unresolved fallback it's the only way forward.
    private func titleField(placeholder: String) -> some View {
        HStack {
            TextField(placeholder, text: $searchQuery)
                .textFieldStyle(OmnibusFieldStyle())
                .autocorrectionDisabled()
                .submitLabel(.search)
                .onSubmit { Task { await search() } }
            Button {
                Task { await search() }
            } label: {
                if isSearching {
                    ProgressView()
                } else {
                    Image(systemName: "magnifyingglass.circle.fill").font(.system(size: 26))
                }
            }
            .disabled(searchQuery.nilIfBlank == nil || isSearching)
            .foregroundStyle(palette.accentColor)
            .accessibilityLabel("Search by title")
        }
    }

    @ViewBuilder
    private var searchResultsList: some View {
        if let results = searchResults {
            if results.isEmpty {
                Text("No books match that title. Check the spelling, or try just the main title.")
                    .font(.ui(13))
                    .foregroundStyle(palette.ink2Color)
            } else {
                VStack(spacing: Spacing.sm) {
                    ForEach(results, id: \.isbn13) { meta in
                        Button {
                            Task { await resolvePick(meta) }
                        } label: {
                            searchResultRow(meta)
                        }
                        .buttonStyle(.plain)
                        .disabled(isResolving)
                        .opacity(isResolving ? 0.55 : 1)
                    }
                }
            }
        }
    }

    /// One pickable candidate: cover, title, authors, and the provider facts
    /// (series, first publish, publisher, pages) when carried.
    private func searchResultRow(_ meta: ExternalBookMeta) -> some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            Group {
                if let cover = meta.coverURL, CheckInFlow.isExternalURL(cover) {
                    ExternalImage(url: cover) { coverPlate(title: meta.title) }
                        .aspectRatio(2.0 / 3.0, contentMode: .fit)
                        .clipShape(RoundedRectangle(cornerRadius: Radius.sm, style: .continuous))
                } else {
                    coverPlate(title: meta.title)
                }
            }
            .frame(width: 44)

            VStack(alignment: .leading, spacing: 3) {
                Text(meta.title)
                    .font(.ui(14, weight: .medium))
                    .foregroundStyle(palette.ink0Color)
                    .multilineTextAlignment(.leading)
                Text(meta.authorDisplay)
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink2Color)
                ForEach(CheckInFlow.detailLines(for: meta), id: \.self) { line in
                    Text(line)
                        .font(.ui(11.5))
                        .foregroundStyle(palette.ink3Color)
                }
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(palette.ink3Color)
                .padding(.top, 4)
        }
        .padding(Spacing.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                .fill(palette.bg1Color)
        )
    }

    private enum DetailsLinkStyle { case filled, quiet }

    @ViewBuilder
    private func detailsLink(uuid: String, style: DetailsLinkStyle) -> some View {
        switch style {
        case .filled:
            Button("Go to Book Details") {
                detailTarget = DetailTarget(uuid: uuid)
            }
            .buttonStyle(FilledButtonStyle())
            .disabled(isWriting)
        case .quiet:
            Button {
                detailTarget = DetailTarget(uuid: uuid)
            } label: {
                Text("Go to Book Details").frame(maxWidth: .infinity)
            }
            .buttonStyle(QuietButtonStyle())
            .disabled(isWriting)
        }
    }

    private func coverPlate(title: String) -> some View {
        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
            .fill(palette.coverFallbackBg.color)
            .aspectRatio(2.0 / 3.0, contentMode: .fit)
            .overlay {
                Text(title.prefix(1).uppercased())
                    .font(.display(28))
                    .foregroundStyle(palette.coverFallbackInk.color.opacity(0.5))
            }
    }

    private func externalCover(_ meta: ExternalBookMeta) -> CheckInSuccess.Cover {
        if let url = meta.coverURL, CheckInFlow.isExternalURL(url) {
            return .external(url: url)
        }
        return .plate
    }

    private func resultCard(
        title: String, authors: [String], cover: CheckInSuccess.Cover,
        badge: String, tint: Color, details: [String] = []
    ) -> some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            Group {
                switch cover {
                case let .library(uuid):
                    BookCover(identity: CoverIdentity(uuid: uuid, title: title, hasCover: true), size: .md)
                case let .external(url):
                    ExternalImage(url: url) { coverPlate(title: title) }
                        .aspectRatio(2.0 / 3.0, contentMode: .fit)
                        .clipShape(RoundedRectangle(cornerRadius: Radius.md, style: .continuous))
                case .plate:
                    coverPlate(title: title)
                }
            }
            .frame(width: 76)

            VStack(alignment: .leading, spacing: 5) {
                Text(badge.uppercased())
                    .font(.monoUI(9, weight: .semibold))
                    .tracking(0.7)
                    .foregroundStyle(tint)
                Text(title)
                    .font(.display(20))
                    .foregroundStyle(palette.ink0Color)
                Text(authors.isEmpty ? "Unknown author" : authors.joined(separator: ", "))
                    .font(.ui(13))
                    .foregroundStyle(palette.ink2Color)
                ForEach(details, id: \.self) { line in
                    Text(line)
                        .font(.ui(12))
                        .foregroundStyle(palette.ink3Color)
                }
            }
            Spacer(minLength: 0)
        }
    }

    private func checkInButton(book: ScanBook, isbn: String?, label: String) -> some View {
        Button {
            Task { await checkIn(book: book, isbn: isbn) }
        } label: {
            if isWriting {
                ProgressView().controlSize(.small)
            } else {
                Text(label)
            }
        }
        .buttonStyle(FilledButtonStyle())
        .disabled(isWriting)
    }

    // MARK: - Actions

    private func resolve(_ raw: String) async {
        let isbn = raw.filter { $0.isNumber || $0 == "X" || $0 == "x" }
        guard isbn.count >= 10 else {
            error = "That doesn't look like an ISBN."
            return
        }
        isResolving = true
        error = nil
        defer { isResolving = false }
        do {
            let result: ScanOutcome = try await APIClient.shared.post(
                "/api/scan/resolve", body: ScanResolveRequest(isbn: isbn)
            )
            withAnimation(Motion.settle) {
                if CheckInFlow.resolveShouldClearSearch(from: stage) {
                    searchQuery = ""
                    searchResults = nil
                }
                stage = .outcome(result)
            }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Back to the scanner for the next book.
    private func restart() {
        withAnimation(Motion.settle) {
            stage = .scan
            manualISBN = ""
            note = ""
            error = nil
            searchQuery = ""
            searchResults = nil
        }
    }

    /// Search the providers by title — the unresolved screen's fallback.
    private func search() async {
        guard let query = searchQuery.nilIfBlank, !isSearching else { return }
        // A resolve can land while this request is still in flight — guard the
        // assignment on the stage/query at request time so a late response
        // can't reintroduce the stale-search bug `resolve(_:)` clears against.
        let startedStage = stage
        let startedQuery = query
        isSearching = true
        error = nil
        defer { isSearching = false }
        do {
            let response: ScanSearchResponse = try await APIClient.shared.post(
                "/api/scan/search", body: ScanSearchRequest(query: query)
            )
            guard
                CheckInFlow.searchResponseShouldApply(
                    startedStage: startedStage, currentStage: stage,
                    startedQuery: startedQuery, currentQuery: searchQuery
                )
            else { return }
            searchResults = response.results
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Resolve a picked candidate against the library — no provider re-lookup,
    /// which could miss on a book the search itself just surfaced.
    private func resolvePick(_ meta: ExternalBookMeta) async {
        guard !isResolving else { return }
        isResolving = true
        error = nil
        defer { isResolving = false }
        do {
            let outcome: ScanOutcome = try await APIClient.shared.post(
                "/api/scan/resolve-meta", body: ResolveMetaRequest(meta: meta)
            )
            Haptics.success()
            withAnimation(Motion.settle) { stage = .outcome(outcome) }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// One write at a time — the second tap of a double-tap lands while the
    /// first POST is still in flight, and must not fire another.
    private func beginWrite() -> Bool {
        guard !isWriting else { return false }
        isWriting = true
        error = nil
        return true
    }

    private func checkIn(book: ScanBook, isbn: String?) async {
        guard beginWrite() else { return }
        defer { isWriting = false }
        do {
            let ref: BookRef = try await APIClient.shared.post(
                "/api/scan/check-in",
                body: CheckInRequest(bookUUID: book.uuid, isbn: isbn, note: note.nilIfBlank)
            )
            Haptics.success()
            // The server answers with the canonical uuid (a merged book
            // resolves to its primary); bust both keys when they differ.
            await OfflineStore.shared.cacheDelete(CacheKey.book(book.uuid))
            if ref.bookUUID != book.uuid {
                await OfflineStore.shared.cacheDelete(CacheKey.book(ref.bookUUID))
            }
            // Check-in fulfills every user's wishlist for the book, so shelf
            // counts and preview covers are stale too.
            await OfflineStore.shared.cacheDelete(CacheKey.shelves)
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews)
            withAnimation(Motion.settle) { stage = .success(CheckInFlow.checkedInSuccess(book: book, ref: ref)) }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func addPhysicalOnly(_ meta: ExternalBookMeta) async {
        guard beginWrite() else { return }
        defer { isWriting = false }
        do {
            let ref: BookRef = try await APIClient.shared.post(
                "/api/scan/physical-only",
                body: AddPhysicalOnlyRequest(meta: meta, note: note.nilIfBlank)
            )
            Haptics.success()
            withAnimation(Motion.settle) { stage = .success(CheckInFlow.addedSuccess(meta: meta, ref: ref)) }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func addWishlist(_ meta: ExternalBookMeta) async {
        guard beginWrite() else { return }
        defer { isWriting = false }
        do {
            let ref: BookRef = try await APIClient.shared.post(
                "/api/scan/wishlist",
                body: WishlistAddRequest(bookUUID: nil, meta: meta, source: .scan)
            )
            Haptics.success()
            // The wishlist is a real shelf whose membership derives from these
            // entries, so its count and preview covers are now stale.
            await OfflineStore.shared.cacheDelete(CacheKey.shelves)
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews)
            withAnimation(Motion.settle) { stage = .success(CheckInFlow.wishlistedSuccess(meta: meta, ref: ref)) }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}

// MARK: - VisionKit scanner

struct BarcodeScannerView: UIViewControllerRepresentable {
    var onScan: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onScan: onScan) }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let controller = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.ean13, .ean8, .upce, .code128])],
            qualityLevel: .balanced,
            recognizesMultipleItems: false,
            isHighFrameRateTrackingEnabled: false,
            isGuidanceEnabled: false,
            isHighlightingEnabled: true
        )
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {
        guard !controller.isScanning else { return }
        try? controller.startScanning()
    }

    static func dismantleUIViewController(
        _ controller: DataScannerViewController, coordinator: Coordinator
    ) {
        controller.stopScanning()
    }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private let onScan: (String) -> Void
        /// The scanner re-reports the same barcode many times a second while
        /// it stays in frame; only the first read should fire.
        private var seen: String?

        init(onScan: @escaping (String) -> Void) {
            self.onScan = onScan
        }

        func dataScanner(
            _ scanner: DataScannerViewController, didAdd items: [RecognizedItem],
            allItems: [RecognizedItem]
        ) {
            for case let .barcode(barcode) in items {
                guard let payload = barcode.payloadStringValue, payload != seen else { continue }
                seen = payload
                onScan(payload)
                return
            }
        }
    }
}
