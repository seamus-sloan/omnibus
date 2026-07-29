//  LibraryPollTests.swift
//  The background poll gate: a tick may only refresh when it can't fight the
//  user (a load in flight, a paginated grid) or waste the radio (offline).

import Testing

@testable import omnibus

struct LibraryPollTests {
    @Test func should_poll_when_online_idle_and_on_first_page() {
        #expect(LibraryModel.shouldPoll(
            isOnline: true, isLoading: false, isLoadingMore: false, hasPaginated: false
        ))
    }

    @Test func should_not_poll_while_offline() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: false, isLoading: false, isLoadingMore: false, hasPaginated: false
        ))
    }

    @Test func should_not_poll_while_a_reload_is_in_flight() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: true, isLoading: true, isLoadingMore: false, hasPaginated: false
        ))
    }

    @Test func should_not_poll_while_a_page_fetch_is_in_flight() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: true, isLoading: false, isLoadingMore: true, hasPaginated: false
        ))
    }

    @Test func should_not_poll_once_the_user_has_paginated() {
        #expect(!LibraryModel.shouldPoll(
            isOnline: true, isLoading: false, isLoadingMore: false, hasPaginated: true
        ))
    }
}
