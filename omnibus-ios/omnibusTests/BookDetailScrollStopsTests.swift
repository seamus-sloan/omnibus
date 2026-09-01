//  BookDetailScrollStopsTests.swift
//  The book-detail scroll-stops account preference: its wire decode (a
//  pre-0092 payload has no key and must land on the off default) and the
//  parked state the Account switch currently renders.

import Foundation
import Testing

@testable import omnibus

struct BookDetailScrollStopsTests {
    @Test func userSummaryDecodeDefaultsScrollStopsWhenAbsent() throws {
        // A pre-0092 server, or a `/me` blob cached from one.
        let json = """
            {"id":1,"username":"alice","is_admin":false,
             "can_upload":false,"can_edit":false,"can_download":true}
            """
        let me = try JSONDecoder().decode(UserSummary.self, from: Data(json.utf8))
        #expect(me.bookDetailScrollStops == false)
    }

    @Test func userSummaryDecodeCarriesScrollStopsWhenSet() throws {
        let json = """
            {"id":1,"username":"alice","is_admin":false,
             "can_upload":false,"can_edit":false,"can_download":true,
             "book_detail_scroll_stops":true}
            """
        let me = try JSONDecoder().decode(UserSummary.self, from: Data(json.utf8))
        #expect(me.bookDetailScrollStops)
    }

    /// The switch ships inert. Pinning it keeps the flag from being lit up
    /// without the book detail page being taught to read the preference —
    /// which would give the reader a control that changes nothing.
    @Test func scrollStopsSwitchIsParkedUntilTheDetailPageReadsIt() {
        #expect(AccountView.scrollStopsParked)
    }
}
