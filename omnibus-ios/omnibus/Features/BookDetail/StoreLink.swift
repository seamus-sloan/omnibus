//  StoreLink.swift
//  Where to buy a wishlisted book: an Amazon search keyed on the ISBN when
//  there is one, else on title + author — the same rule as the web page's
//  "Find a copy" link (`find_a_copy_url` in frontend's book_detail/physical).

import Foundation

enum StoreLink {
    /// Amazon search URL for the book.
    static func searchURL(isbn: String?, title: String, author: String) -> URL? {
        let key = isbn?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let query = key.isEmpty
            ? "\(title) \(author)".trimmingCharacters(in: .whitespacesAndNewlines)
            : key

        var components = URLComponents(string: "https://www.amazon.com/s")
        components?.queryItems = [URLQueryItem(name: "k", value: query)]
        return components?.url
    }

    /// The same search addressed to the Amazon app's URL scheme, which mirrors
    /// web paths verbatim. Opening it succeeds only when the app is installed,
    /// so callers fall back to the https URL when the open is refused.
    static func appURL(for web: URL) -> URL? {
        let swapped = web.absoluteString.replacingOccurrences(
            of: "https://www.amazon.com/",
            with: "com.amazon.mobile.shopping.web://amazon.com/"
        )
        return URL(string: swapped)
    }
}
