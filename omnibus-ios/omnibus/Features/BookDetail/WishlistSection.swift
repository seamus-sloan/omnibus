//  WishlistSection.swift
//  The book-detail wishlist card: what is being tracked, where to buy it, and
//  the way back off the wishlist. Mirrors the web page's rail tracking card
//  (`BdWishlistTrackingCard` in frontend/src/pages/book_detail/physical.rs) —
//  same three facts, same two actions — behind a confirmation the web button
//  doesn't ask for, since a mis-tap here has nothing to undo it with.

import SwiftUI

struct WishlistSection: View {
    let book: Book
    let entry: WishlistEntry
    /// Run once the server has dropped the entry, so the page can clear it.
    var onRemoved: () -> Void

    @Environment(\.palette) private var palette
    @Environment(\.openURL) private var openURL
    private var connectivity = Connectivity.shared

    @State private var showRemoveConfirm = false
    @State private var busy = false
    @State private var error: String?

    init(book: Book, entry: WishlistEntry, onRemoved: @escaping () -> Void) {
        self.book = book
        self.entry = entry
        self.onRemoved = onRemoved
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Wishlist")

            Text(Self.trackingLine(added: Format.relative(unix: entry.addedAt), source: entry.source))
                .font(.ui(13))
                .foregroundStyle(palette.ink2Color)

            Button {
                Haptics.tap()
                openStore()
            } label: {
                Label("Find a store", systemImage: "storefront")
            }
            .buttonStyle(FilledButtonStyle(prominent: false))

            Text("Opens an Amazon search for this book.")
                .font(.ui(12.5))
                .foregroundStyle(palette.ink3Color)

            removeAction
        }
        .confirmationDialog(
            "Remove from wishlist?", isPresented: $showRemoveConfirm, titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) { remove() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(Self.confirmMessage(hasFiles: book.hasEbook || book.hasAudiobook))
        }
    }

    /// The way off the wishlist, plus whatever is standing in its way. Offline
    /// it is disabled rather than left to fail after the fact (rule 08), so it
    /// says why instead of going quiet.
    private var removeAction: some View {
        VStack(alignment: .leading, spacing: 5) {
            Button(busy ? "Removing…" : "Remove from wishlist") {
                Haptics.tap()
                showRemoveConfirm = true
            }
            .font(.ui(13, weight: .medium))
            .foregroundStyle(palette.badColor)
            .disabled(busy || !connectivity.isOnline)
            .accessibilityIdentifier("wishlist-remove")

            if let error {
                Text(error)
                    .font(.ui(12.5))
                    .foregroundStyle(palette.badColor)
                    .accessibilityIdentifier("wishlist-error")
            } else if !connectivity.isOnline {
                Text("Removing needs a connection.")
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink3Color)
            }
        }
        .padding(.top, Spacing.xs)
    }

    private func remove() {
        busy = true
        error = nil
        Task {
            do {
                try await UserDataService.removeWishlistEntry(uuid: book.uuid)
                busy = false
                Haptics.success()
                // The card goes with the entry, so the page owns the state
                // rather than this view holding a removed row on screen.
                withAnimation(Motion.settle) { onRemoved() }
            } catch {
                busy = false
                self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
            }
        }
    }

    /// Open the store search in the Amazon app when it's installed, else the
    /// browser. The app's URL scheme is tried first because an https link only
    /// reaches the app when Amazon's universal-link registration covers the
    /// path — the scheme is the deterministic door.
    private func openStore() {
        guard let web = StoreLink.searchURL(
            isbn: book.isbn13, title: book.displayTitle, author: book.authorDisplay
        ) else { return }

        guard let app = StoreLink.appURL(for: web) else {
            openURL(web)
            return
        }
        openURL(app) { accepted in
            if !accepted { openURL(web) }
        }
    }

    // MARK: - Copy

    // Pure and `nonisolated` so the suite can assert the wording without a
    // main-actor hop, the way `LibraryModel.railShelves` is.

    /// The card's one-line summary, worded like the web tracking card so the
    /// two surfaces describe an entry the same way.
    nonisolated static func trackingLine(added: String, source: WishlistSource) -> String {
        "Tracking this title · added \(added) from \(sourceLabel(source))"
    }

    /// Human phrase for where an entry came from. Mirrors `source_label` on
    /// the web page.
    nonisolated static func sourceLabel(_ source: WishlistSource) -> String {
        switch source {
        case .scan: "a scan"
        case .detail: "this page"
        case .manual: "manual entry"
        }
    }

    /// What the confirmation warns about, which differs by what else the
    /// library holds. A book with files keeps everything but the tracking. One
    /// without is reachable *because* of the entry: browse hides a fileless
    /// book with no physical copy, and the add-to-shelf picker reads that same
    /// listing, so the wishlist is the only place it surfaces.
    nonisolated static func confirmMessage(hasFiles: Bool) -> String {
        hasFiles
            ? "This stops tracking a physical copy. The book stays in your library."
            : "Your library holds no files for this book, so the wishlist is the only place it shows up."
    }
}
