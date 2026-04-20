# F0.6 — Library filesystem convention

**Phase 0 · Foundations** · **Priority:** P0

Define the on-disk layout Omnibus writes. Keep the read path permissive so users can point Omnibus at any existing folder (including a Calibre library) without reorganizing first — but **do not promise Calibre round-trip compatibility.**

## Objective

Omnibus canonical layout:

```
<library_root>/
  <author-slug>/
    <title-slug>/
      <title-slug>.epub          # one or more format files
      <title-slug>.m4b
      cover.jpg                  # optional sidecar; preferred over embedded if present
```

No `metadata.opf` sidecar. No `(id)` suffix on folder names. Display-name authors, not `Lastname, Firstname`. Metadata lives in the DB; the filesystem stays human-readable and tool-agnostic.

## User / business value

1. Uploads ([F5.3](5-3-uploads.md)) produce a predictable, human-browsable layout instead of dumping files at the library root.
2. If a user stops using Omnibus, the folder still makes sense in Finder, Syncthing, or a future self-hosted tool.
3. Removing OPF/Calibre-compat ambition eliminates a large class of edge cases before they ship.

## Scope

- **Read path — fully layout-tolerant.** Scanner walks arbitrary trees recursively (current behavior preserved). Metadata comes from each ebook's internal OPF (we already parse this). A Calibre library dropped in *works* via the normal scan path — we just don't advertise it as a first-class migration.
- **Cover sidecar, opportunistic.** If `cover.jpg` / `cover.png` sits next to an ebook, prefer it over the embedded cover — faster (no epub re-open) and typically higher quality. This is a filename convention, not an OPF thing; zero interop cost.
- **Ignore `metadata.opf` sidecars.** Every field they contain is also in the epub's internal OPF. The only deltas are Calibre-specific edits a user made in Calibre desktop — if they want those edits preserved, they should stay on Calibre. Parsing the sidecar would add a reconcile-with-embedded codepath we don't need.
- **Write path — canonical layout, slugged filenames.** Uploads ([F5.3](5-3-uploads.md)) compute `<library_root>/<author-slug>/<title-slug>/<title-slug>.<ext>` from the metadata extracted from the uploaded file. Collision → suffix ` (2)` on the title folder. Never overwrite.
- **No renaming on metadata edit.** Metadata overrides live in the DB (`books.metadata_overrides` JSON), never mutate folder names or file contents. Explicitly rejects Calibre-Web's folder-rename-on-edit path (racy with readers and scanners).

## Calibre interop decision

We are *not* a drop-in replacement for Calibre's library format. Reasons:

1. Faithful Calibre layout requires the `(id)` suffix on title folders, `Lastname, Firstname` author folders via `opf:file-as`, Calibre's sanitization quirks (`:` → `_`), and `calibre:title_sort` / `calibre:series` / custom-column conventions. Copying all of that inherits 2006 desktop-era design decisions.
2. The sidecar `metadata.opf` is bit-for-bit the OPF already inside the epub, plus two `calibre:*` meta tags. It's not a portable metadata standard — it's Calibre's state file. Treating it as authoritative input means reconciling two sources of truth for every book.
3. Scanning an existing Calibre library already works today via the embedded-epub-OPF path. That's a nice-to-have, not a feature we should stake a roadmap claim on.
4. If cross-tool migration becomes a demand signal post-v1.0, ship a dedicated one-shot importer (`omnibus import-calibre <path>`) that reads Calibre's `metadata.db` directly — cleaner than making the live scanner shaped like Calibre.

## Technical considerations

- New `library_layout` module under `frontend/src/` with `canonical_path(metadata) -> PathBuf` and `sidecar_cover_for(ebook_path) -> Option<PathBuf>`.
- Unit tests against fixture trees that include (a) canonical Omnibus layout, (b) flat dump, (c) a minimal Calibre-shaped tree to confirm tolerant scan.

## Slug rules

ASCII-fold, lowercase, non-alphanumerics → `-`, collapse runs, trim to 80 chars. Document this; users will `ls` these folders. Keep the display name (with punctuation, case, unicode) in the DB — the slug is purely a filesystem artifact.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) — need `authors` + `title` columns to compute slugs.

## Risks

- Users expect Calibre interop and are disappointed. Mitigation: explicit in docs, and the tolerant scan means their library still works — just not as "their Calibre library."
- Slug collisions across unicode-equivalent titles. Mitigated by collision-suffix on write.

## Open questions

- Do we ship an optional "reorganize library into canonical layout" admin action? **Recommendation:** no for v1.0 — destructive, no user demand yet.
- Do we want an OPF *export* action (write a fresh OPF when a user downloads a book) as a courtesy to users leaving Omnibus? **Recommendation:** defer. Zero cost to skip; easy to add later.

---

[← Back to roadmap summary](0-0-summary.md)
