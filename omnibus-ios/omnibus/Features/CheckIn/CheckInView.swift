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
    @State private var outcome: ScanOutcome?
    @State private var isResolving = false
    @State private var error: String?
    @State private var scannerAvailable = DataScannerViewController.isSupported
        && DataScannerViewController.isAvailable
    @State private var note = ""
    @State private var didComplete = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if outcome == nil {
                    scannerSection
                } else {
                    outcomeSection
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
    }

    // MARK: - Scanning

    private var scannerSection: some View {
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
                    message: "Enter the ISBN by hand instead."
                )
            }

            VStack(alignment: .leading, spacing: Spacing.sm) {
                Text("Or enter an ISBN")
                    .font(.ui(13, weight: .medium))
                    .foregroundStyle(palette.ink2Color)
                HStack {
                    TextField("9780000000000", text: $manualISBN)
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
                }
            }
            .screenPadding()

            if let error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.ui(13))
                    .foregroundStyle(palette.badColor)
                    .screenPadding()
            }

            Spacer()
        }
    }

    // MARK: - Outcome

    @ViewBuilder
    private var outcomeSection: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                switch outcome {
                case let .alreadyOwned(book):
                    resultCard(
                        title: book.title, authors: book.authors, uuid: book.uuid,
                        badge: "Already on your shelf", tint: palette.okColor
                    )
                    Text("You've already checked in a physical copy of this book.")
                        .font(.ui(14))
                        .foregroundStyle(palette.ink2Color)
                    checkInButton(uuid: book.uuid, isbn: book.isbn, label: "Add another copy")

                case let .onWishlist(book):
                    resultCard(
                        title: book.title, authors: book.authors, uuid: book.uuid,
                        badge: "On your wishlist", tint: palette.accentColor
                    )
                    Text("Checking a copy in clears this book from your wishlist.")
                        .font(.ui(14))
                        .foregroundStyle(palette.ink2Color)
                    noteField
                    checkInButton(uuid: book.uuid, isbn: book.isbn, label: "Check in this copy")

                case let .inLibraryUnowned(book):
                    resultCard(
                        title: book.title, authors: book.authors, uuid: book.uuid,
                        badge: "In your library", tint: palette.accentColor
                    )
                    noteField
                    checkInButton(uuid: book.uuid, isbn: book.isbn, label: "Check in this copy")

                case let .closeMatch(book, scanned):
                    resultCard(
                        title: book.title, authors: book.authors, uuid: book.uuid,
                        badge: "Possible match", tint: palette.warnColor
                    )
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
                    }
                    .padding(Spacing.md)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                            .fill(palette.bg1Color)
                    )
                    noteField
                    checkInButton(uuid: book.uuid, isbn: scanned.isbn13, label: "Yes, same book — check in")
                    Button("No, add as a new physical book") {
                        Task { await addPhysicalOnly(scanned) }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .frame(maxWidth: .infinity)

                case let .notInLibrary(online):
                    resultCard(
                        title: online.title, authors: online.authors, uuid: nil,
                        badge: "Not in your library", tint: palette.ink2Color,
                        remoteCover: online.coverURL
                    )
                    noteField
                    Button("Add as physical book") {
                        Task { await addPhysicalOnly(online) }
                    }
                    .buttonStyle(FilledButtonStyle())
                    Button("Add to wishlist") {
                        Task { await addWishlist(online) }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .frame(maxWidth: .infinity)

                case .unresolved, .none:
                    EmptyStateView(
                        icon: "questionmark.circle",
                        title: "Couldn't identify that book",
                        message: "Neither your library nor the online providers recognised that ISBN."
                    )
                }

                if didComplete {
                    Label("Saved", systemImage: "checkmark.circle.fill")
                        .font(.ui(14, weight: .medium))
                        .foregroundStyle(palette.okColor)
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
                    withAnimation {
                        outcome = nil
                        manualISBN = ""
                        note = ""
                        didComplete = false
                        error = nil
                    }
                }
                .font(.ui(14))
                .foregroundStyle(palette.accentColor)
                .frame(maxWidth: .infinity)
                .padding(.top, Spacing.sm)
            }
            .screenPadding()
            .padding(.vertical, Spacing.lg)
        }
    }

    private var noteField: some View {
        TextField("Note (optional)", text: $note, axis: .vertical)
            .textFieldStyle(OmnibusFieldStyle())
            .lineLimit(1...3)
    }

    private func resultCard(
        title: String, authors: [String], uuid: String?,
        badge: String, tint: Color, remoteCover: String? = nil
    ) -> some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            Group {
                if let uuid {
                    BookCover(identity: CoverIdentity(uuid: uuid, title: title, hasCover: true), size: .md)
                } else {
                    RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                        .fill(palette.coverFallbackBg.color)
                        .aspectRatio(2.0 / 3.0, contentMode: .fit)
                        .overlay {
                            Text(title.prefix(1).uppercased())
                                .font(.display(28))
                                .foregroundStyle(palette.coverFallbackInk.color.opacity(0.5))
                        }
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
            }
            Spacer(minLength: 0)
        }
    }

    private func checkInButton(uuid: String, isbn: String?, label: String) -> some View {
        Button(label) {
            Task { await checkIn(uuid: uuid, isbn: isbn) }
        }
        .buttonStyle(FilledButtonStyle())
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
            withAnimation { outcome = result }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Drop the previous write's result before starting another. Neither banner
    /// names the action it came from, so a leftover "Saved" beside a fresh error
    /// reads as if the write that just failed had succeeded.
    private func beginWrite() {
        error = nil
        withAnimation { didComplete = false }
    }

    private func checkIn(uuid: String, isbn: String?) async {
        beginWrite()
        do {
            let _: Empty = try await APIClient.shared.post(
                "/api/scan/check-in",
                body: CheckInRequest(bookUUID: uuid, isbn: isbn, note: note.nilIfBlank)
            )
            Haptics.success()
            withAnimation { didComplete = true }
            await OfflineStore.shared.cacheDelete(CacheKey.book(uuid))
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func addPhysicalOnly(_ meta: ExternalBookMeta) async {
        beginWrite()
        do {
            let _: Empty = try await APIClient.shared.post(
                "/api/scan/physical-only",
                body: AddPhysicalOnlyRequest(meta: meta, note: note.nilIfBlank)
            )
            Haptics.success()
            withAnimation { didComplete = true }
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func addWishlist(_ meta: ExternalBookMeta) async {
        beginWrite()
        do {
            let _: Empty = try await APIClient.shared.post(
                "/api/scan/wishlist",
                body: WishlistAddRequest(bookUUID: nil, meta: meta, source: .scan)
            )
            Haptics.success()
            withAnimation { didComplete = true }
            // The wishlist is a real shelf whose membership derives from these
            // entries, so its count and preview covers are now stale.
            await OfflineStore.shared.cacheDelete(CacheKey.shelves)
            await OfflineStore.shared.cacheDelete(CacheKey.shelfPreviews)
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
