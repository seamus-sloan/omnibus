//  LibraryPollTests.swift
//  The background poll gate: a tick may only refresh when it can't fight the
//  user (a load in flight, a paginated grid) or waste the radio (offline).

import Testing

@testable import omnibus

struct LibraryPollTests {
    @Test func pollsWhenOnlineIdleAndOnFirstPage() {
        #expect(LibraryModel.shouldPoll(
            isOnline: true, isLoading: false, isLoadingMore: false, hasPaginated: false
        ))
    }

    @Test func skipsWhileOffline() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: false, isLoading: false, isLoadingMore: false, hasPaginated: false
        ))
    }

    @Test func skipsWhileAReloadIsInFlight() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: true, isLoading: true, isLoadingMore: false, hasPaginated: false
        ))
    }

    @Test func skipsWhileAPageFetchIsInFlight() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: true, isLoading: false, isLoadingMore: true, hasPaginated: false
        ))
    }

    @Test func skipsOnceTheUserHasPaginated() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: true, isLoading: false, isLoadingMore: false, hasPaginated: true
        ))
    }
}
