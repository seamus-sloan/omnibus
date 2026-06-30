# F4.4 — Kobo annotation import (USB)

**Phase 4 · Device sync** · **Priority:** P2

Pull highlights, notes, and bookmarks **off** a Kobo and into Omnibus's
`highlights` / `bookmarks` tables via a browser-driven USB import — the
device → Omnibus direction that the wireless [F4.1](4-1-kobo-sync.md)
sync protocol cannot carry.

## Why this is a separate initiative from F4.1

[F4.1](4-1-kobo-sync.md) is **Omnibus → Kobo**: it serves books and
round-trips *reading state* (position, read status, statistics). It does
**not** bring user annotations back. This is a hard limit of the Kobo
wireless protocol as implemented by the de-facto reference
([Calibre-Web's `cps/kobo.py`](https://github.com/janeczku/calibre-web/blob/master/cps/kobo.py)):
there is **no annotations endpoint**, and the `/kobo/v1/library/<uuid>/state`
handler processes only `CurrentBookmark` (position/progress), `Statistics`,
and `StatusInfo`. Highlights, notes, and quotes never travel over sync.

The only way to recover device-side annotations is to read them off the
Kobo's filesystem over USB. That's a fundamentally different mechanism
(local file access + parsing, not an HTTP protocol), so it lives here.

## Objective

A browser-driven import that reads a Kobo's annotation store over USB,
maps each annotation to an Omnibus `book_uuid`, and upserts into the
`highlights` (and where applicable `bookmarks`) tables — idempotently, so
re-importing is a no-op.

## User / business value

Closes the round-trip: highlights you make *on the Kobo* show up in
Omnibus's reader drawers and feed [F5.7 journal & quote cards](5-7-journal-quote-cards.md).
Without it, annotations are stranded on the device — a recurring
Calibre-Web complaint ([#2155](https://github.com/janeczku/calibre-web/issues/2155)).

## Architecture: web client, not server

**The Kobo plugs into the machine running the browser, not the server.**
A Kobo mounts as a **USB mass-storage drive** on whatever computer it's
cabled to — for a self-hoster that's their laptop, never the server box
(NAS/VPS). So the import is a **web-client** flow; the Omnibus server
never touches USB.

1. **File access in the browser** — the user selects the Kobo's
   `KoboReader.sqlite` via `<input type="file">` (works in *all* browsers,
   including Firefox/Safari and the simplest to build). The File System
   Access API (`showOpenFilePicker` / `showDirectoryPicker`) is a
   Chromium-only upgrade worth deferring until a "remember my Kobo and
   re-sync on reconnect" feature is wanted.
2. **Parse server-side** — upload the `.sqlite` to a server function /
   `POST /api/import/kobo`, open it with `rusqlite` against a throwaway
   connection, read the `Bookmark` table, and upsert into `highlights`.
   Keeps all logic in `db/` alongside existing patterns and tests, and
   avoids shipping a SQLite-WASM lib into the WASM client. (Client-side
   WASM parse — extract rows, POST only the highlights — is the
   privacy-nicer alternative; deferred as heavier to build.)

The Kobo's `Bookmark` table holds the data we want: `Text` (the
highlighted passage), `Annotation` (the user's note), `ContentID`
(device-local book path), `StartContainerPath` / `EndContainerPath`
(location anchors), `DateCreated`.

## ⚠️ Data-loss warnings (read before building)

USB import is read-only and safe **if** the importer only ever *opens*
`KoboReader.sqlite` for reading. The destructive risks are adjacent, and
worth calling out so nobody wires them in:

- **Never write back to `KoboReader.sqlite`.** It is the device's live
  master DB. A partial or schema-mismatched write can brick the library
  view or wipe reading state. Import is strictly device → Omnibus; there
  is no reverse path in this initiative.
- **Eject before unplug.** Reading the file while the OS has the volume
  mounted is fine, but the user must eject cleanly — yanking a mounted
  Kobo mid-read can corrupt the SQLite file. Surface this in the UI.
- **Do not confuse this with wireless sync, which *deletes* annotations.**
  Pointing a Kobo's *sync* endpoint at a self-hosted server that doesn't
  speak the annotation channel causes the device to **clear its own local
  annotations** ([#2610](https://github.com/janeczku/calibre-web/issues/2610),
  [#1783](https://github.com/janeczku/calibre-web/issues/1783)). If F4.1
  ships first, **import via this USB flow before the device's first
  wireless sync**, or the annotations may already be gone. This ordering
  hazard belongs in the F4.1 risks too.
- **Treat the uploaded `.sqlite` as untrusted input.** Parse it in a
  throwaway connection, never attach it to the app DB, and delete the
  temp file after import.

## Technical considerations

- **Book matching is the hard part**, not file access. `ContentID` is a
  device-local path string, not an Omnibus uuid. Map it back to
  `book_uuid` — ideally by threading the uuid through the
  [F4.1](4-1-kobo-sync.md) download filename so the path is recoverable.
  Unmatched annotations should surface as "unlinked" (mirror the
  unlinked-annotations UI in [F3.2](3-2-ratings-journaling.md)) rather
  than being dropped.
- **Idempotent upsert.** Key on a stable tuple
  (`ContentID` + `StartContainerPath` + `DateCreated`) so a second import
  is a no-op — same spirit as the `_norm` backfill in
  [migration 0016](../../db/migrations/) being idempotent.
- **`.annot` is a secondary path, not the primary one.** Adobe Digital
  Editions sidecar files at `/Digital Editions/Annotations/books/`
  (XHTML-parsed-as-XML) cover ADE-managed *sideloaded EPUBs*, **not**
  KEPUB. Since F4.1 serves KEPUB (via kepubify), highlights land in
  `KoboReader.sqlite`, not `.annot`. Keep `.annot` parsing as a fallback
  for plain-EPUB workflows; chapter/page recovery from `.annot` is
  unreliable (opaque location ids).
- **Desktop browser only.** Phones can't host USB mass storage, so this
  is inherently a laptop/desktop flow — fine, since that's where a Kobo
  gets plugged in anyway. The mobile app is out of scope.
- **Hidden folder UX.** `KoboReader.sqlite` lives in the hidden `.kobo/`
  dir at the drive root. The picker needs a one-line hint to reveal
  hidden files (macOS `Cmd+Shift+.`; Windows "show hidden files").

## Acceptance criteria

- A web import page accepts a `KoboReader.sqlite` upload and reports a
  per-book count of highlights/notes imported.
- Imported highlights appear in the reader's highlights drawer
  ([F2.4b](2-4b-reader-interactive.md)) for matched books.
- Re-running the same import imports zero new rows (idempotent).
- Annotations for books not in the library surface as "unlinked", not
  silently dropped.
- No code path writes to the uploaded DB or to the device.

## Dependencies

- [F0.3 Auth](0-3-auth.md) — import is per-user.
- [F2.4b Reader interactive](2-4b-reader-interactive.md) — the
  `highlights` table and drawers this populates.
- [F4.1 Native Kobo sync](4-1-kobo-sync.md) — the download route that
  should embed `book_uuid` in the filename for matching; and the sync
  ordering hazard above.

## Risks

- **Book matching reliability.** If `ContentID` can't be mapped to a
  uuid, imports land as unlinked. Designing F4.1's download filename with
  this in mind is the mitigation.
- **Kobo schema drift.** `KoboReader.sqlite`'s `Bookmark` columns are
  reverse-engineered, not contracted; a firmware update could shift them.
  Pin to the columns we read and fail loudly on absence.
- **Browser file-access support.** `<input type="file">` is universal;
  the File System Access API upgrade is Chromium-only — keep the baseline
  flow on the portable primitive.

---

[← Back to roadmap summary](0-0-summary.md)
