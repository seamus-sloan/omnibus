# F3.4 — Reading stats

**Phase 3 · Personalization** · **Priority:** P2

Year-in-review style reading stats: hours read, pages turned, books finished, longest streak, busiest week, top tag.

## Objective

A `/stats` route that visualizes the user's reading and listening activity over a configurable window (default: this calendar year). Headline numbers, a heatmap of daily activity, top genres/authors by hours, and a "finished books" rail. Built on top of progress events from [F2.1](2-1-progress-sync.md).

## User / business value

Stats are the moment users *show* the app to a friend. They're also the closing-loop on the reading habit — seeing yourself read 2 hours/day for 8 weeks is the reason a user stays self-hosted instead of going back to Kindle. Calibre-Web has no equivalent.

## Technical considerations

- Backed by the progress-sync events table from [F2.1](2-1-progress-sync.md). One row per session (start_at, end_at, book_id, format, words_read_estimate).
- All aggregations are SQL: `GROUP BY date(start_at)` for the heatmap, `SUM(end_at - start_at)` for hours, `COUNT(DISTINCT book_id) WHERE progress = 1.0` for finishes.
- Cache aggregate results per-user in memory for 60 s — a fresh page reload should reflect a just-finished session, but expensive queries shouldn't re-run on every signal poll.
- UI uses Atrium primitives from [F1.7](1-7-atrium-design-system.md); the heatmap and bar charts are pure CSS (no charting library).
- No new schema beyond what F2.1 lands.

## Dependencies

- [F2.1 Progress sync service](2-1-progress-sync.md) — events table is the data source.
- [F1.7 Atrium design system](1-7-atrium-design-system.md) — visual primitives.

## Acceptance criteria

- `/stats` renders for any authenticated user. Empty state is friendly (no progress events yet).
- Date-range picker (Year / 6 months / All time) re-queries instantly.
- Cold-load < 250 ms on a library with 10k progress events.

## Related

- [Atrium design doc](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)
