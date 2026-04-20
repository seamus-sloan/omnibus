# F0.5 — Background worker primitive

**Phase 0 · Foundations** · **Priority:** P0

A single `tokio::task::JoinSet` + `Semaphore` + typed task enum driving scans, thumbnails, email, and (future) conversion.

## Objective

Provide one shared async-task primitive so every feature that needs background work (scan, thumbnail generation, email send, format conversion) posts to the same queue and respects the same concurrency cap.

## User / business value

Avoids Calibre-Web's single-`WorkerThread` ceiling (see [performance pain points](../calibre-inspection/3-performance-pain-points.md)), where a slow `ebook-convert` blocks every other task. Keeps the web path responsive while CPU-bound work runs.

## Technical considerations

- Typed `enum Task { Scan(..), Thumbnail(..), SendEmail(..), ConvertFormat(..) }` — designing this enum conservatively is the main design cost (additions easy, renames painful).
- `JoinSet` + `Semaphore::new(max_concurrency)` where concurrency is per-task-type (e.g. `max(1, num_cpus / 2)` for conversions; more for thumbnails).
- **Per-resource fairness guards**, not just global concurrency. When a future task type works against multiple independent resources (e.g. per-library scan, per-provider metadata lookup), one slow resource shouldn't starve the others — hold a per-resource guard inside the task so concurrent work on *different* resources proceeds while the same resource queues serially. ABS serializes all podcast downloads through a single `currentDownload` slot and this is its most-complained-about backlog — the shape of the warning applies even though Omnibus won't fetch media from the web ([ABS recommendation #8](../audiobookshelf-inspection/7-recommendations.md), [PodcastManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/PodcastManager.js)).
- In-memory queue initially — acceptable because we own the single-process model. Persist to a `background_tasks` table when [F5.2 observability](5-2-observability.md) arrives so admins can see status and history.
- Task API: `worker.post(Task::Thumbnail { … }) -> TaskId`; optional `worker.await_completion(id)`.

## Dependencies

None.

## Risks

- Task enum design locks in early. Mitigate by reviewing it before the first non-trivial task type ships.

---

[← Back to roadmap summary](0-0-summary.md)
