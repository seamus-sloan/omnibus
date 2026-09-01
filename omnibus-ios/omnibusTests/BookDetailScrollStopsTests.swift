//  BookDetailScrollStopsTests.swift
//  The book-detail scroll-stops account preference: its wire decode (a
//  pre-0092 payload has no key and must land on the off default) and the
//  flow map the off-default Option B layout scrolls by.

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

    // MARK: - Flow geometry

    @Test func flowMapRestsWithTheCoverWholeAtTheTop() {
        let map = DetailRead.flowMap(offset: 0, restTop: 603)
        #expect(map.lift == 0)
        #expect(map.page == 0)
        #expect(!map.past)
    }

    @Test func flowMapLiftsAcrossTheRunToTheBodySnapPosition() {
        // The body snaps navPeek short of the cover's end — fully lifted,
        // and already "past": the strip is what covers the peeking art.
        let map = DetailRead.flowMap(offset: 603 - DetailRead.flowNavPeek, restTop: 603)
        #expect(map.lift == 1)
        #expect(map.past)
    }

    @Test func flowMapStaysUnpastWhileTheCoverMostlyShows() {
        let map = DetailRead.flowMap(offset: 200, restTop: 603)
        #expect(map.lift > 0)
        #expect(!map.past)
    }

    @Test func flowMapWashesTheArtAsTheListRunsOn() {
        let map = DetailRead.flowMap(offset: 603 * 2, restTop: 603)
        #expect(map.past)
        #expect(map.page > 1)
    }
}
