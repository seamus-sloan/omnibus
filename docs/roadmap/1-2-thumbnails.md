# F1.2 — Thumbnail pipeline

**Phase 1 · Browse & discovery** · **Priority:** P0

On-demand cover resizing, WebP cache on disk, three sizes, responsive `srcset` delivery.

## Objective

Serve cover thumbnails at 3 sizes (small/medium/large) cached as WebP under `<data_dir>/thumbs/<book_id>_<size>.webp`. Generate on first request, not on a nightly schedule.

## User / business value

Cover grids are the single highest-bandwidth path in the product. Closes [gap G5](0-0-summary.md#gaps). WebP at three sizes delivers ~30% smaller payloads than JPEG at equivalent quality, and `srcset` lets the browser pick the right size instead of downscaling a 2400px cover for a 180px grid cell.

This sidesteps Calibre-Web's scheduled-only pipeline (see [calibre-inspection §3](../calibre-inspection/3-performance-pain-points.md)), where users browsing a freshly imported library before the nightly task runs get the full-size original rescaled by the browser — the common "slow covers" complaint.

## Technical considerations

- `image` crate for decode + resize, `webp` crate for encode.
- Cache invalidation keyed on `books.last_modified` — regenerate if the book row's timestamp is newer than the thumbnail's `mtime`.
- Generation runs on the [F0.5 worker](0-5-background-worker.md) so the web request returns immediately (serve placeholder → 304 after generate).
- Prefer `cover.jpg` sidecar next to the ebook over the embedded cover ([F0.6](0-6-library-filesystem.md)).
- LRU eviction past a configurable cap (default ~5 GB) to keep disk footprint bounded on 100k-book libraries.
- **Re-examine [F0.6](0-6-library-filesystem.md) sidecar lookup cost while we have a perf harness.** [`sidecar_cover_for`](../../db/src/library_layout.rs) currently does up to 2× `read_dir` per epub (per-stem then folder-level). Acceptable for canonical Omnibus layouts (1 epub per folder), but may show up on flat-dump libraries with thousands of files in one folder. F1.2 is the natural moment to measure: thumbnail generation iterates every book and stresses the same code path. If measurement shows it matters, swap in a fast-path that checks the expected exact-case names directly via `is_file()` before falling back to the case-insensitive `read_dir`. Tracked in [omnibus#52](https://github.com/seamus-sloan/omnibus/issues/52).

## Dependencies

- [F0.5 Background worker](0-5-background-worker.md).
- Cover-storage decision: filesystem cache under `<data_dir>/thumbs/`, not the current BLOB table (see [summary §7 open questions](0-0-summary.md#open-questions)).

## Risks

- Disk footprint at scale. Mitigated by LRU eviction cap.
- Cold-cache latency on first load — acceptable because subsequent loads hit the cache; mitigated further by pre-warming on scan.

---

[← Back to roadmap summary](0-0-summary.md)
