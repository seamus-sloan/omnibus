//  ImageCacheInvalidationTests.swift
//  What a cover write invalidates, and what may put it back.
//
//  The case that must not regress: a cover fetch already in flight when the
//  cover is replaced. `LibraryService.uploadCover` invalidates every cover key
//  correctly, but the reply to that earlier fetch lands afterwards, and
//  committing it re-creates the entry the write just deleted — with the
//  superseded art, and a fresh mtime that also restarts the revalidation
//  window. That is how a library tile kept showing the previous cover while
//  the detail hero showed the new one (#2547), after #2540 had already fixed
//  the unrelated validator-less path behind #2539.

import Foundation
import Testing
import UIKit

@testable import omnibus

@Suite("Image cache invalidation")
struct ImageCacheInvalidationTests {
    /// A solid-colour PNG standing in for one cover's bytes.
    private func cover(red: CGFloat, green: CGFloat, blue: CGFloat) -> (UIImage, Data) {
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 8, height: 12))
        let image = renderer.image { context in
            UIColor(red: red, green: green, blue: blue, alpha: 1).setFill()
            context.fill(CGRect(x: 0, y: 0, width: 8, height: 12))
        }
        return (image, image.pngData() ?? Data())
    }

    /// A cache of this test's own. Several of these invalidate and clear,
    /// which against `ImageCache.shared` would delete entries a sibling test
    /// running in parallel is asserting on.
    private func makeCache() -> ImageCache {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("cover-cache-tests/\(UUID().uuidString)", isDirectory: true)
        return ImageCache(directory: directory)
    }

    private let key = "/api/thumbs/book-uuid/md"

    @Test("a fetch in flight when the key is invalidated does not put the old cover back")
    func aLateStoreIsRefused() async {
        let cache = makeCache()
        let (oldCover, oldBytes) = cover(red: 1, green: 0, blue: 0)

        // A library tile is showing the cover the book had before the upload.
        await cache.store(oldCover, data: oldBytes, for: key, etag: "\"old\"")

        // A fetch for that same cover is issued — it will resolve after the
        // upload lands.
        let generation = await cache.generation(for: key)

        // The upload lands and drops every cover key for the book.
        await cache.invalidate(key)
        #expect(await cache.image(for: key) == nil)

        // The earlier fetch now resolves and tries to commit.
        let committed = await cache.store(
            oldCover, data: oldBytes, for: key, etag: "\"old\"", generation: generation
        )

        #expect(committed == false)
        #expect(await cache.image(for: key) == nil, "the invalidated entry must stay gone")
    }

    @Test("a fetch that raced nothing still commits")
    func anUnracedStoreCommits() async {
        let cache = makeCache()
        let (art, bytes) = cover(red: 0, green: 0, blue: 1)

        let generation = await cache.generation(for: key)
        let committed = await cache.store(
            art, data: bytes, for: key, etag: "\"v1\"", generation: generation
        )

        #expect(committed)
        #expect(await cache.image(for: key) != nil)
    }

    @Test("clearing the whole cache also refuses a fetch already in flight")
    func clearDiskRefusesALateStore() async {
        let cache = makeCache()
        let (art, bytes) = cover(red: 0, green: 1, blue: 0)

        // No per-key invalidation has ever been recorded for this key, so a
        // per-key counter alone would not catch this one.
        let generation = await cache.generation(for: key)
        await cache.clearDisk()
        let committed = await cache.store(
            art, data: bytes, for: key, etag: "\"v1\"", generation: generation
        )

        #expect(committed == false)
        #expect(await cache.image(for: key) == nil)
    }

    @Test("bytes that came from no fetch are committed without a generation")
    func storeWithoutAGenerationCommits() async {
        // The escape hatch: a caller holding bytes that cannot have raced an
        // invalidation — nothing was asked of the server — passes no
        // generation and is not second-guessed.
        let cache = makeCache()
        let (art, bytes) = cover(red: 1, green: 1, blue: 0)

        await cache.invalidate(key)
        #expect(await cache.store(art, data: bytes, for: key, etag: "\"v1\""))
        #expect(await cache.image(for: key) != nil)
    }

    @Test("a key's generation only ever rises")
    func generationsAreMonotonic() async {
        // `clearDisk` adds to the per-key counts rather than resetting them,
        // so a generation captured before it can never be matched again by a
        // later sequence of per-key invalidations.
        let cache = makeCache()

        let start = await cache.generation(for: key)
        await cache.invalidate(key)
        let afterKey = await cache.generation(for: key)
        await cache.clearDisk()
        let afterClear = await cache.generation(for: key)

        #expect(afterKey > start)
        #expect(afterClear > afterKey)
    }
}
