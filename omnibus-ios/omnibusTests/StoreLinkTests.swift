//  StoreLinkTests.swift
//  The wishlist "Find a store" target: which query the Amazon search gets,
//  and the app-scheme swap the button tries before falling back to the
//  browser. Mirrors the web page's `find_a_copy_url` tests so the two
//  surfaces can't quietly diverge on what a store search means.

import Foundation
import Testing

@testable import omnibus

@Suite struct StoreLinkTests {
    @Test func searchesByISBNWhenPresent() {
        let url = StoreLink.searchURL(
            isbn: "9780451524935", title: "1984", author: "George Orwell")
        #expect(url?.absoluteString == "https://www.amazon.com/s?k=9780451524935")
    }

    @Test func fallsBackToTitleAndAuthorWithoutAnISBN() {
        let url = StoreLink.searchURL(
            isbn: nil, title: "Brave New World", author: "Aldous Huxley")
        #expect(
            url?.absoluteString
                == "https://www.amazon.com/s?k=Brave%20New%20World%20Aldous%20Huxley")
    }

    @Test func treatsAWhitespaceOnlyISBNAsAbsent() {
        // Newline included deliberately: web trims with Rust `trim()`, which
        // strips newlines too, and the two surfaces must build the same query.
        let url = StoreLink.searchURL(isbn: " \n ", title: "1984", author: "George Orwell")
        #expect(url?.absoluteString == "https://www.amazon.com/s?k=1984%20George%20Orwell")
    }

    @Test func appURLSwapsToTheAmazonScheme() throws {
        let web = try #require(
            StoreLink.searchURL(isbn: "9780451524935", title: "1984", author: "George Orwell"))
        #expect(
            StoreLink.appURL(for: web)?.absoluteString
                == "com.amazon.mobile.shopping.web://amazon.com/s?k=9780451524935")
    }
}

/// The wire contract for `GET /api/physical/{uuid}/wishlist`: an entry object,
/// or a bare JSON `null` for a book the caller isn't tracking. The `null` case
/// is load-bearing — the cache layer round-trips the optional through the same
/// decoder, so a top-level-fragment regression would read every book as
/// untracked.
@Suite struct WishlistEntryCodecTests {
    @Test func decodesAnEntryFromTheServerShape() throws {
        let json = """
            {"id":7,"user_id":3,"book_uuid":"abc-123","added_at":1723200000,"source":"scan"}
            """
        let entry = try JSONDecoder().decode(WishlistEntry?.self, from: Data(json.utf8))
        #expect(entry?.id == 7)
        #expect(entry?.userID == 3)
        #expect(entry?.bookUUID == "abc-123")
        #expect(entry?.addedAt == 1_723_200_000)
        #expect(entry?.source == .scan)
    }

    @Test func decodesABareNullAsNotTracked() throws {
        let entry = try JSONDecoder().decode(WishlistEntry?.self, from: Data("null".utf8))
        #expect(entry == nil)
    }
}
