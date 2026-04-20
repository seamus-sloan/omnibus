# F5.1 — Metadata edit

**Phase 5 · Admin & hygiene** · **Priority:** P1

In-UI editing of book metadata, stored as DB overrides, never mutating disk.

## Objective

Admin (and optionally edit-permitted users) can edit title, authors, series, tags, description, and replace the cover image, all from the book detail page. Edits persist to a `books.metadata_overrides` JSON column and are merged on read.

## User / business value

Closes [gap G7](0-0-summary.md#gaps). Self-hosted libraries are messy; fixing metadata without shelling into the server is table stakes. Absence of this is Calibre-Web's biggest retention hook — see [calibre-inspection §2](../calibre-inspection/2-feature-inventory.md).

## Technical considerations

- **Edits go to DB only, never to disk.** No folder renames, no file mutation, no OPF rewrites. See [recommendations #7, #8](../calibre-inspection/7-recommendations.md) — Calibre-Web's folder-rename-on-edit path races with readers and scanners and is the wrong shape to copy.
- `books.metadata_overrides` JSON merged on read: scanned values form the base; override keys win.
- Cover replace writes to the [F1.2 thumbnail cache](1-2-thumbnails.md) and bumps `last_modified` to invalidate downstream caches.
- OPF export exists only as a per-book download action (courtesy to users leaving Omnibus), never as a source-of-truth sync target.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).
- [F0.3 Auth](0-3-auth.md) (requires `can_edit` permission).

## Risks

- Merge rules between scanned values and overrides need to be explicit and tested — especially for m2m relations (authors, tags). A tag list override should replace, not append. Cover specifically: when a user uploads a new cover, it wins over both the sidecar and the embedded cover.

---

[← Back to roadmap summary](0-0-summary.md)
