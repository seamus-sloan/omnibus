# F5.8 — OPF Export

**Depends on:** F5.1 (metadata edit)

## Problem

Omnibus stores metadata overrides in a database table (`metadata_overrides`). If a user leaves Omnibus or wants to share their corrections with another tool (Calibre, Audiobookshelf, etc.), those overrides are locked inside the DB with no portable export path.

## Solution

Add a one-shot OPF export that bakes DB overrides back into portable OPF sidecar files on disk.

### Surfaces

1. **Per-book action** on the book edit page (`/books/:id/edit`): a "Export to OPF" button that writes a `metadata.opf` sidecar next to the source file.
2. **Bulk action** in the admin settings page: an "Export all OPF" button that writes sidecar files for every book that has overrides.

### Behavior

- Writes standard EPUB OPF 2.0 XML (`<package>` / `<metadata>` / Dublin Core elements).
- Merges scanned + override values so the exported file is a complete snapshot.
- File is written to the canonical sidecar path: `{book_dir}/metadata.opf`.
- If a sidecar already exists, the server backs it up as `metadata.opf.bak` before overwriting.
- Cover override (if present) is copied as `cover.jpg` alongside the OPF.
- The export is strictly one-shot and write-only. It does not set up a sync relationship — the DB remains the source of truth after export.

### API

- `POST /api/ebooks/{id}/export-opf` — single-book export. Returns `{ path, backed_up }`.
- `POST /api/admin/export-all-opf` — admin-only bulk export. Returns `{ exported, skipped, errors }`.
- RPC equivalents: `rpc_export_opf(book_id)`, `rpc_export_all_opf()`.

### Non-goals

- No import from OPF (that's part of the indexer pipeline and already exists).
- No continuous sync. This is a point-in-time snapshot, not a bidirectional bridge.
- No UI for diff / merge against an existing sidecar — that's closer to F5.1 Screen B (fetch & merge).

## Acceptance criteria

- [ ] Single-book export writes valid OPF 2.0 XML with all override fields merged.
- [ ] Bulk export touches only books with active overrides.
- [ ] Existing sidecar is backed up before overwrite.
- [ ] Cover override is exported alongside OPF.
- [ ] Non-admin users with `can_edit` can export single books; bulk is admin-only.
- [ ] Unit tests for OPF XML generation.
- [ ] Integration tests for both REST endpoints.
