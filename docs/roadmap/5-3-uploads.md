# F5.3 — Uploads

**Phase 5 · Admin & hygiene** · **Priority:** P2

Multi-file web uploads that land in the canonical Omnibus layout.

## Objective

Admin (and upload-permitted users) can drag-and-drop ebooks/audiobooks into the web UI. Files are parsed for metadata, written into the layout defined by [F0.6](0-6-library-filesystem.md), and indexed.

## User / business value

v1 #2. Demoted from Phase 1 because most self-hosters already ship books via Syncthing / rsync / NFS — in-UI upload is convenience, not a blocker. Shipping it after the foundations means it slots cleanly into the canonical filesystem layout without a rework.

## Technical considerations

- Accept `.epub`, `.m4a`, `.m4b` for v1.0. Other formats (pdf, cbz, fb2) post-v1.0 — keep the accept list narrow until we have a story for them.
- Parse uploaded metadata first, **then** compute the target path via `library_layout::canonical_path(metadata)` from [F0.6](0-6-library-filesystem.md).
- Collision: title folder gets ` (2)` / ` (3)` suffix; never overwrite.
- Large files stream to disk via `axum::extract::Multipart` — never buffer.
- Post-write, enqueue an index job through the [F0.5 worker](0-5-background-worker.md).

## Dependencies

- [F0.3 Auth](0-3-auth.md) (requires `can_upload` permission).
- [F0.5 Background worker](0-5-background-worker.md).
- [F0.6 Library filesystem convention](0-6-library-filesystem.md).

## Changes from v1

- Demoted from Phase 1 to Phase 5.
- Target layout is now Omnibus-canonical (F0.6), not "somewhere inside the library path."

---

[← Back to roadmap summary](0-0-summary.md)
