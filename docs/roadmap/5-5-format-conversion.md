# F5.5 — Format conversion (optional, deferred)

**Phase 5 · Admin & hygiene** · **Priority:** P3

Shell out to Calibre's `ebook-convert` for on-demand format conversion. Post-v1.0.

## Objective

Surface a "Convert to…" action on the book detail page and in admin bulk actions that shells out to Calibre's `ebook-convert` binary when present on the host. No bundled converter.

## User / business value

Closes a real Calibre-Web parity gap for power users. Gives them AZW3, PDF, TXT, FB2, etc. without leaving Omnibus. **Post-v1.0 unless user demand appears** — most users asking for format conversion already have Calibre installed and can convert there.

## Scope

- **In scope:** generic `ebook-convert` shell-out for any format pair Calibre supports; per-conversion async job via the [F0.5 worker](0-5-background-worker.md); result stored as a new row in `book_files` — formats coexist for the same work, clean because of [F0.1](0-1-schema-refactor.md).
- **Already shipped in [F4.1](4-1-kobo-sync.md):** EPUB → KEPUB via kepubify. This initiative reuses that worker infrastructure rather than duplicating it.
- **Out of scope for v1.x:** CBZ/CBR generation (different audience); audiobook transcoding (m4a ↔ mp3); any Rust-native converter (see [assumption A7](0-0-summary.md#assumptions)).

## Technical considerations

- Config flag `ebook_convert_path` auto-detected on startup, overridable in admin ([F5.4](5-4-admin-panel.md)).
- Task type `ConvertFormat { book_id, source_format, target_format }` posted to the F0.5 worker.
- Generous timeout — some conversions take minutes.
- Surface progress/completion through [F5.2 observability](5-2-observability.md).
- Semaphore must cap concurrent conversions (`max(1, num_cpus / 2)`) — `ebook-convert` is CPU- and memory-heavy.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).
- [F0.5 Background worker](0-5-background-worker.md).
- [F5.2 Observability](5-2-observability.md).
- [F5.4 Admin panel](5-4-admin-panel.md) (config flag UI).

## Risks

- Conversion quality is frequently poor (PDF → EPUB is famously bad); users will blame Omnibus for Calibre's output. Mitigation: docs clearly label this as a pass-through to Calibre.
- Calibre is a ~300 MB dependency. Keep it strictly optional at runtime; document the install path for Docker / Nix deployments.

## Open question

Should Omnibus ship a "convert on upload" mode (e.g. "always convert uploaded AZW3 to EPUB for browser reading")? **Recommendation:** no — keeping formats as the user uploaded them preserves fidelity; convert on demand instead.

---

[← Back to roadmap summary](0-0-summary.md)
