//  WishlistSectionTests.swift
//  The book-detail wishlist card's copy: how an entry describes itself, and
//  what the removal confirmation warns about.
//
//  The source phrases mirror `source_label` on the web page, so the two
//  surfaces can't drift on what "from a scan" means. The confirmation is the
//  part worth pinning: a fileless book is only *visible* because of its
//  wishlist entry — the browse query hides a fileless book with no physical
//  copy — so the warning has to differ from the one a book with files gets.

import Testing

@testable import omnibus

@Suite("Wishlist card copy")
struct WishlistSectionTests {
    @Test("names where a scanned entry came from")
    func labelsAScan() {
        #expect(WishlistSection.sourceLabel(.scan) == "a scan")
    }

    @Test("names the detail page as its own source")
    func labelsTheDetailPage() {
        #expect(WishlistSection.sourceLabel(.detail) == "this page")
    }

    @Test("names a hand-made entry")
    func labelsManualEntry() {
        #expect(WishlistSection.sourceLabel(.manual) == "manual entry")
    }

    @Test("states what is tracked, when, and from where")
    func trackingLineReadsAsOneSentence() {
        #expect(
            WishlistSection.trackingLine(added: "3d ago", source: .scan)
                == "Tracking this title · added 3d ago from a scan"
        )
    }

    @Test("a book with files is told it keeps them")
    func confirmMessageReassuresWhenFilesRemain() {
        let message = WishlistSection.confirmMessage(hasFiles: true)
        #expect(message == "This stops tracking a physical copy. The book stays in your library.")
    }

    @Test("a book with no files is warned the wishlist is all there is")
    func confirmMessageWarnsWhenTheEntryIsTheOnlyThingHoldingIt() {
        // Removing the entry takes the last way back to a book the browse
        // query already hides — the reader must be told before, not after.
        let message = WishlistSection.confirmMessage(hasFiles: false)
        #expect(message != WishlistSection.confirmMessage(hasFiles: true))
        #expect(message.contains("the only place it shows up"))
    }
}
