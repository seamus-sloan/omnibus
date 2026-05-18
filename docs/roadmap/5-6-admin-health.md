# F5.6 — Admin server health

**Phase 5 · Admin & hygiene** · **Priority:** P2

A dedicated admin surface for "what is the server doing right now" — index status, recent scans, worker queue depth, FTS index health, storage utilization, last failed task.

## Objective

Split from [F5.4 Admin panel](5-4-admin-panel.md) because health and user-management have different audiences. Health is what an operator opens when something feels wrong; user-management is what they open when adding a household member. The Atrium handoff dedicates a full screen to health, so it gets its own page.

Surfaces:
- **Index status** — last scan time per library, books indexed, books failed, current worker state (idle / scanning / reindexing).
- **Worker queue** — per-task-type concurrency, current depth, recent completion times. Backed by `db::worker` primitive from [F0.5](0-5-background-worker.md).
- **FTS index** — row count, last rebuild, last query latency p50/p95.
- **Storage** — disk usage per library, thumbnail cache size vs `OMNIBUS_THUMBS_CAP_BYTES`.
- **Last errors** — last 20 error log lines, with the offending book/file linked.

## User / business value

Self-hosted operators have no Datadog, no PagerDuty. The admin health page IS observability for them. Calibre-Web's lack of this is the most-cited reason its operators move to AudioBookShelf.

## Technical considerations

- Read-only page. No write endpoints; no schema changes.
- Worker queue depth is exposed by extending `db::worker::Worker` with a `pub fn metrics() -> WorkerMetrics` accessor returning current depth and recent timings.
- "Last errors" reads from an in-memory ring buffer that the tracing layer feeds. Bounded to ~200 entries. Eventually [F5.2 Observability](5-2-observability.md) replaces this with a real log sink, but the ring buffer is useful even after.
- Per-route gated to AdminUser via the existing extractor from [F0.7](0-7-route-authorization.md).

## Dependencies

- [F0.5 Background worker primitive](0-5-background-worker.md).
- [F0.7 Per-route authorization](0-7-route-authorization.md).
- [F1.7 Atrium design system](1-7-atrium-design-system.md) — visual primitives.

## Acceptance criteria

- `/admin/health` renders the five sections above for an admin user.
- Non-admin users get a 403.
- Page polls (or uses SSE) every 5 s; visible counts update without manual reload.

## Related

- [F5.4 Admin panel](5-4-admin-panel.md) — adjacent surface for user management.
- [Atrium design doc](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)
