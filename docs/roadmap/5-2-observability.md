# F5.2 — Observability

**Phase 5 · Admin & hygiene** · **Priority:** P1

Structured logs, Prometheus metrics, admin log viewer, background-task dashboard.

## Objective

`tracing`-based structured logs to disk (JSON), Prometheus scrape endpoint at `/metrics`, admin UI for browsing logs and inspecting background-task history.

## User / business value

Closes [gap G8](0-0-summary.md#gaps). First production bug is free of guesswork; scheduled-task visibility is a Calibre-Web feature we should not regress on — their admin surface shows task status, which is the one thing operators actually check.

## Technical considerations

- `tracing` + `tracing-subscriber` with both stderr and rolling-file JSON sinks.
- `axum-prometheus` for HTTP method/status/path histograms.
- `background_tasks` table populated by the [F0.5 worker](0-5-background-worker.md) once it starts persisting — feeds the admin dashboard. Status, started/finished timestamps, error message.
- Log viewer is just a paginated query over the JSON logs with filter-by-level/module/time-range.

## Dependencies

- [F0.5 Background worker](0-5-background-worker.md).

---

[← Back to roadmap summary](0-0-summary.md)
