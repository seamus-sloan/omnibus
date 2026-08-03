//  LibraryShelfRailTests.swift
//  Which shelves reach the landing rail.
//
//  `GET /api/shelves` answers with everything the caller may *see*, not
//  everything they own — a public shelf from another account, and for an admin
//  every account's. The rail was rendering that list verbatim, so the shelves
//  section on the landing screen filled up with other people's.

import Testing

@testable import omnibus

private func preview(
    id: Int64 = 1, owner: Int64 = 7, kind: ShelfKind = .manual, name: String = "Reread",
    books: Int64 = 3
) -> ShelfPreview {
    ShelfPreview(
        shelf: ShelfSummary(
            id: id, ownerUserId: owner, ownerUsername: "owner-\(owner)", kind: kind,
            name: name, visibility: .public, accent: nil, bookCount: books
        ),
        covers: []
    )
}

@Suite("Landing shelves rail")
struct LibraryShelfRailTests {
    @Test("keeps the signed-in user's own shelves")
    func keepsOwnShelves() {
        let mine = preview(id: 1, owner: 7)
        #expect(LibraryModel.railShelves([mine], userId: 7).map(\.id) == [1])
    }

    @Test("drops another account's public shelf")
    func dropsOtherUsersShelves() {
        // Visible, and browsable behind All — but not "yours", which is what
        // the rail claims to be.
        let mine = preview(id: 1, owner: 7)
        let theirs = preview(id: 2, owner: 9, name: "Someone else's")
        let rail = LibraryModel.railShelves([mine, theirs], userId: 7)
        #expect(rail.map(\.id) == [1])
    }

    @Test("an admin sees only their own, not the whole server's")
    func dropsEveryOtherAccountForAnAdmin() {
        // An admin's read carries every account's shelves; the landing screen
        // is not the place that inventory belongs.
        let rail = LibraryModel.railShelves(
            [preview(id: 1, owner: 1), preview(id: 2, owner: 2), preview(id: 3, owner: 3)],
            userId: 1
        )
        #expect(rail.map(\.id) == [1])
    }

    @Test("drops a wishlist nothing has been added to")
    func dropsAnEmptyWishlist() {
        // Every account gets one whether or not it is used, so an unused one is
        // a card that says nothing.
        let empty = preview(id: 1, owner: 7, kind: .wishlist, name: "Wishlist", books: 0)
        #expect(LibraryModel.railShelves([empty], userId: 7).isEmpty)
    }

    @Test("keeps a wishlist once it holds something")
    func keepsAStockedWishlist() {
        let stocked = preview(id: 1, owner: 7, kind: .wishlist, name: "Wishlist", books: 2)
        #expect(LibraryModel.railShelves([stocked], userId: 7).map(\.id) == [1])
    }

    @Test("keeps an empty shelf that is not a wishlist")
    func keepsAnEmptyManualShelf() {
        // You made it deliberately and are about to fill it; hiding it would
        // read as the create having failed.
        let empty = preview(id: 1, owner: 7, kind: .manual, books: 0)
        #expect(LibraryModel.railShelves([empty], userId: 7).map(\.id) == [1])
    }

    @Test("shows the superset while the identity is still unconfirmed")
    func keepsEverythingWhenTheUserIsUnknown() {
        // `setServer` reaches `.ready` before `confirmIdentity` returns, so the
        // library can render with no id in hand. Blanking a rail that is about
        // to be right is worse than briefly showing one shelf too many.
        let rail = LibraryModel.railShelves(
            [preview(id: 1, owner: 7), preview(id: 2, owner: 9)], userId: nil
        )
        #expect(rail.map(\.id) == [1, 2])
    }

    @Test("preserves the server's ordering")
    func preservesOrdering() {
        // The rail must not reshuffle between loads — same reason the preview
        // fetch restores the server's order after its task group.
        let rail = LibraryModel.railShelves(
            [preview(id: 3, owner: 7), preview(id: 1, owner: 9), preview(id: 2, owner: 7)],
            userId: 7
        )
        #expect(rail.map(\.id) == [3, 2])
    }
}
