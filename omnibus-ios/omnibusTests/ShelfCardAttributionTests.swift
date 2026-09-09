//  ShelfCardAttributionTests.swift
//  Whose shelf a card says it is.
//
//  The rail and the shelves index both carry shelves the viewer does not own,
//  so a card that names no owner reads as the viewer's own. Mirrors
//  `shows_owner_attribution` on the web rail — the two surfaces describe the
//  same shelves and must not disagree about whose they are.

import Testing

@testable import omnibus

@Suite("Shelf card owner attribution")
struct ShelfCardAttributionTests {
    @Test("attributes a shelf owned by someone else")
    func attributesAnotherAccountsShelf() {
        #expect(
            ShelfCard.showsOwnerAttribution(viewerId: 7, ownerUserId: 9, kind: .manual)
        )
    }

    @Test("leaves your own shelf unattributed")
    func leavesYourOwnUnattributed() {
        #expect(
            !ShelfCard.showsOwnerAttribution(viewerId: 7, ownerUserId: 7, kind: .manual)
        )
    }

    @Test("withholds attribution while the viewer is unknown")
    func withholdsWhileTheViewerIsUnresolved() {
        // Guessing "someone else's" before `confirmIdentity` returns would
        // label the viewer's own shelves as borrowed for that moment.
        #expect(
            !ShelfCard.showsOwnerAttribution(viewerId: nil, ownerUserId: 9, kind: .manual)
        )
    }

    @Test("never attributes a wishlist")
    func neverAttributesAWishlist() {
        // Its name already opens with the owner, so the suffix would repeat it.
        #expect(
            !ShelfCard.showsOwnerAttribution(viewerId: 7, ownerUserId: 9, kind: .wishlist)
        )
    }
}
