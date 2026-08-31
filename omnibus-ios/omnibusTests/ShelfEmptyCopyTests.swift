//  ShelfEmptyCopyTests.swift
//  What an empty shelf says about itself, per kind.
//
//  The three kinds fill by three different mechanisms — by hand, by a rule, by
//  checking a copy in — and only the first has an action to offer here. The
//  page used to give every one of them the smart shelf's line, so a wishlist
//  with nothing on it reported that no books matched conditions it does not
//  have. `nil` is the fourth case and the one that regressed in the field:
//  offline, before the detail read lands, the kind is unknown, and the page
//  must describe the shelf without inventing machinery for it.

import Testing

@testable import omnibus

@Suite("Empty shelf copy")
struct ShelfEmptyCopyTests {
    @Test("offers to fill a manual shelf, which is the only kind you fill here")
    func manualShelfOffersAnAction() {
        let copy = ShelfEmptyCopy.forKind(.manual)
        #expect(copy.actionTitle == "Add books")
        #expect(copy.title == "Nothing on this shelf")
    }

    @Test("explains a smart shelf's rules instead of offering an action it can't take")
    func smartShelfExplainsItsRules() {
        let copy = ShelfEmptyCopy.forKind(.smart)
        #expect(copy.actionTitle == nil)
        #expect(copy.message.contains("rules"))
    }

    @Test("tells the wishlist to check a copy in, not that nothing matched conditions")
    func wishlistSpeaksOfCheckingIn() {
        let copy = ShelfEmptyCopy.forKind(.wishlist)
        #expect(copy.actionTitle == nil)
        #expect(copy.message.contains("Check in"))
        #expect(!copy.message.contains("rules"))
    }

    @Test("claims no mechanism when the kind is still unknown")
    func unknownKindStaysNeutral() {
        let copy = ShelfEmptyCopy.forKind(nil)
        #expect(copy.actionTitle == nil)
        #expect(!copy.message.contains("rules"))
        #expect(!copy.message.contains("Check in"))
    }

    @Test("agrees with itself on number when the members haven't reached the device")
    func unreachableCopyAgreesOnNumber() {
        let one = ShelfEmptyCopy.unreachable(count: 1)
        #expect(one.contains("one book"))
        #expect(one.contains("it hasn't"))
        #expect(!one.contains("they haven't"))

        let many = ShelfEmptyCopy.unreachable(count: 4)
        #expect(many.contains("4 books"))
        #expect(many.contains("they haven't"))
    }

    @Test("every kind says something, and never repeats the smart shelf's line")
    func noKindBorrowsAnother() {
        let kinds: [ShelfKind?] = [.manual, .smart, .wishlist, nil]
        let messages = kinds.map { ShelfEmptyCopy.forKind($0).message }
        #expect(messages.allSatisfy { !$0.isEmpty })
        #expect(Set(messages).count == messages.count)
    }
}

@Suite("Shelf meta line")
struct ShelfMetaLineTests {
    private func shelf(kind: ShelfKind, count: Int64 = 0,
                       visibility: ShelfVisibility = .private) -> ShelfSummary
    {
        ShelfSummary(
            id: 1, ownerUserId: 1, ownerUsername: "reader", kind: kind,
            name: "A shelf", visibility: visibility, accent: nil, bookCount: count
        )
    }

    @Test("names the wishlist as a wishlist rather than as a manual shelf")
    func wishlistNamesItself() {
        #expect(ShelfDetailView.metaLine(shelf(kind: .wishlist)) == "0 books · Wishlist")
    }

    @Test("names a manual shelf")
    func manualNamesItself() {
        #expect(ShelfDetailView.metaLine(shelf(kind: .manual, count: 1)) == "1 book · Manual")
    }

    @Test("names a smart shelf, and marks a public one")
    func smartNamesItselfAndItsVisibility() {
        #expect(
            ShelfDetailView.metaLine(shelf(kind: .smart, count: 3, visibility: .public))
                == "3 books · Smart · Public"
        )
    }
}
