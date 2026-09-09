//  LibraryShelfRailTests.swift
//  Which shelves reach the landing rail.
//
//  `GET /api/shelves` answers with everything the caller may *see*, not
//  everything they own — a public shelf from another account, and for an admin
//  every account's private ones too. Rendering that verbatim filled the rail
//  with other people's; restricting it to shelves you own emptied it for
//  everyone who had never made one, hiding the only real shelf on a shared
//  instance. The line that holds is *deliberate and shared*: an auto-issued
//  wishlist is neither.

import Testing

@testable import omnibus

private func preview(
    id: Int64 = 1, owner: Int64 = 7, kind: ShelfKind = .manual, name: String = "Reread",
    books: Int64 = 3, visibility: ShelfVisibility = .public
) -> ShelfPreview {
    ShelfPreview(
        shelf: ShelfSummary(
            id: id, ownerUserId: owner, ownerUsername: "owner-\(owner)", kind: kind,
            name: name, visibility: visibility, accent: nil, bookCount: books
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

    @Test("keeps another account's public shelf")
    func keepsOtherUsersPublicShelves() {
        // The report this rule exists for: on a shared instance one reader's
        // public shelf was the only real shelf on the box, and everyone else
        // saw an empty rail.
        let mine = preview(id: 1, owner: 7)
        let theirs = preview(id: 2, owner: 9, name: "Someone else's")
        let rail = LibraryModel.railShelves([mine, theirs], userId: 7)
        #expect(rail.map(\.id) == [1, 2])
    }

    @Test("keeps another account's public shelf when you own none")
    func keepsOtherUsersPublicShelfWithNoneOfYourOwn() {
        // The exact shape of the report: your only shelf is the wishlist you
        // were issued and never used, so the old rule left nothing at all.
        let mine = preview(id: 1, owner: 7, kind: .wishlist, name: "Wishlist", books: 0)
        let theirs = preview(id: 2, owner: 9, kind: .smart, name: "Someone else's")
        let rail = LibraryModel.railShelves([mine, theirs], userId: 7)
        #expect(rail.map(\.id) == [2])
    }

    @Test("drops another account's private shelf")
    func dropsOtherUsersPrivateShelves() {
        // An admin's read carries every account's private shelves. The rail is
        // a browse surface, not a moderation one.
        let rail = LibraryModel.railShelves(
            [
                preview(id: 1, owner: 1),
                preview(id: 2, owner: 2, visibility: .private),
                preview(id: 3, owner: 3, visibility: .private),
            ],
            userId: 1
        )
        #expect(rail.map(\.id) == [1])
    }

    @Test("keeps your own private shelf")
    func keepsYourOwnPrivateShelf() {
        // Private means private *from other people*, not from you.
        let mine = preview(id: 1, owner: 7, visibility: .private)
        #expect(LibraryModel.railShelves([mine], userId: 7).map(\.id) == [1])
    }

    @Test("drops another account's wishlist even when it is stocked")
    func dropsOtherUsersWishlists() {
        // Every account is issued one, so carrying other people's is how the
        // rail filled with shelves nobody chose to make.
        let theirs = preview(id: 2, owner: 9, kind: .wishlist, name: "owner-9's Wishlist", books: 12)
        #expect(LibraryModel.railShelves([theirs], userId: 7).isEmpty)
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

    @Test("shows the public shelves while the identity is still unconfirmed")
    func keepsPublicShelvesWhenTheUserIsUnknown() {
        // `setServer` reaches `.ready` before `confirmIdentity` returns, so the
        // library can render with no id in hand. Nothing counts as yours for
        // that moment, which leaves the public shelves — enough that the rail
        // is never blank while it waits.
        let rail = LibraryModel.railShelves(
            [preview(id: 1, owner: 7), preview(id: 2, owner: 9)], userId: nil
        )
        #expect(rail.map(\.id) == [1, 2])
    }

    @Test("holds back private shelves while the identity is still unconfirmed")
    func withholdsPrivateShelvesWhenTheUserIsUnknown() {
        // Counting everything as yours until the id lands would flash another
        // account's private shelf to an admin for the length of the
        // confirmation. Yours reappears the moment the id resolves.
        let rail = LibraryModel.railShelves(
            [
                preview(id: 1, owner: 7, visibility: .private),
                preview(id: 2, owner: 9, kind: .wishlist, name: "owner-9's Wishlist", books: 4),
                preview(id: 3, owner: 9),
            ],
            userId: nil
        )
        #expect(rail.map(\.id) == [3])
    }

    @Test("preserves the server's ordering")
    func preservesOrdering() {
        // The rail must not reshuffle between loads — same reason the preview
        // fetch restores the server's order after its task group.
        let rail = LibraryModel.railShelves(
            [preview(id: 3, owner: 7), preview(id: 1, owner: 9), preview(id: 2, owner: 7)],
            userId: 7
        )
        #expect(rail.map(\.id) == [3, 1, 2])
    }
}
