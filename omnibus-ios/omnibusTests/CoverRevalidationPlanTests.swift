//  CoverRevalidationPlanTests.swift
//  When a cached cover checks back with the server, and how.
//
//  The case that must not regress is an entry holding no validator. Treating
//  "cannot ask cheaply" as "never ask" left such an entry pinned to its bytes
//  for the life of the install — which is how a library tile and the book
//  detail hero came to show different covers for the same book (#2539). The
//  hero reads the full-cover route, which always publishes an ETag; a tile
//  fetched before its thumbnail existed was served the stand-in cover, which
//  did not.

import Foundation
import Testing

@testable import omnibus

@Suite("Cover revalidation plan")
struct CoverRevalidationPlanTests {
    /// Comfortably inside the five-minute window.
    private let fresh: TimeInterval = 10
    /// Comfortably past it.
    private let stale: TimeInterval = 600

    @Test("a fresh entry is not checked, validator or no validator")
    func freshEntriesAreSkipped() {
        // Without a window, scrolling a grid would fire one request per
        // visible cover every time it came back into view.
        #expect(ImageCache.plan(age: fresh, etag: "\"v1\"") == .skip)
        #expect(ImageCache.plan(age: fresh, etag: nil) == .skip)
    }

    @Test("a stale entry with a validator asks conditionally")
    func staleEntriesWithAValidatorAskConditionally() {
        #expect(ImageCache.plan(age: stale, etag: "\"v1\"") == .conditional(etag: "\"v1\""))
    }

    @Test("a stale entry with no validator asks outright")
    func staleEntriesWithoutAValidatorAskUnconditionally() {
        // The regression: this used to be `.skip`, and nothing else ever
        // restored the validator, so the entry never asked again.
        #expect(ImageCache.plan(age: stale, etag: nil) == .unconditional)
    }

    @Test("an empty validator counts as none rather than being offered")
    func anEmptyValidatorIsNotOffered() {
        // A zero-byte sidecar is what a truncated write leaves behind.
        // Offering it as `If-None-Match` asks a question no server can answer.
        #expect(ImageCache.plan(age: stale, etag: "") == .unconditional)
    }

    @Test("the window boundary is inclusive")
    func theWindowBoundaryIsInclusive() {
        // Exactly at the window the entry is due — otherwise an entry landing
        // precisely on it waits a whole further window.
        #expect(ImageCache.plan(age: 300, etag: "\"v1\"") == .conditional(etag: "\"v1\""))
        #expect(ImageCache.plan(age: 299, etag: "\"v1\"") == .skip)
    }
}
