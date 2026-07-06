# F4.1 — Native Kobo sync (Omnibus → Kobo)

**Phase 4 · Device sync** · **Priority:** P1

Implement the `/kobo/v1/*` protocol natively, including EPUB → KEPUB conversion via kepubify.

> [!NOTE]
> **Delivery: two stages.** This initiative ships in two parts to isolate the
> wireless protocol's annotation-loss hazard (see [Risks](#risks)):
>
> - **Stage 1 — wired KEPUB sideload (shipped).** A **Send to Kobo** button on
>   each book delivers the book as KEPUB (via kepubify) to a device plugged into
>   the *client* machine. On Chromium (Chrome/Edge) it writes the file straight
>   onto the mounted Kobo drive via the File System Access API (remembering the
>   directory handle so repeat sends need no prompt); other browsers fall back to
>   a plain download the user copies over. No tokens, no protocol, no wireless
>   sync — and **no data-loss risk**, since dropping a loose file on the drive
>   root never touches `KoboReader.sqlite` or the annotation channel (the write
>   is the safe mirror of the [F4.4](4-4-kobo-annotation-import.md) USB *read*).
>   User guide: [docs/kobo.md](../kobo.md). This is the KEPUB **Sub-scope** below,
>   promoted to ship first.
> - **Stage 2 — wireless sync (deferred).** The `/kobo/v1/*` protocol described
>   in the rest of this page. Deferred to a later initiative; settled design
>   decisions are captured under [Deferred: wireless sync](#deferred-wireless-sync).

**Direction & scope.** This is the **Omnibus → Kobo** half: serving books to the device and round-tripping *reading state* (position, read status, statistics). It does **not** bring user annotations (highlights/notes/quotes) back from the device — the Kobo wireless protocol has no annotation channel (see [Technical considerations](#technical-considerations)). Pulling Kobo-side annotations into Omnibus is a separate USB-import initiative: [F4.4 Kobo annotation import](4-4-kobo-annotation-import.md).

## Objective

Implement `/kobo/v1/library/sync`, `/kobo/v1/library/<uuid>/metadata`, `/kobo/v1/library/<uuid>/state`, `/kobo/v1/library/tags`, and `download/*` endpoints. See [calibre-inspection §6](../calibre-inspection/6-api-surface.md) for the route inventory.

## User / business value

Closes [gap G9](0-0-summary.md#gaps). Native Kobo sync is a **far superior UX** to OPDS on Kobo devices: background sync, reading-state round-trip, shelves-as-tags, no manual re-browse. Calibre-Web's implementation is the de-facto reference — users arriving from Calibre-Web expect this as table stakes. Promoted ahead of OPDS in the phasing because it's the better UX for the platform that matters most.

## Sub-scope: EPUB → KEPUB via kepubify

Kobo devices render plain EPUB but with measurably slower page turns than KEPUB; shipping the plain file is leaving UX on the table.

- Detect [`kepubify`](https://github.com/pgaskin/kepubify) on `PATH` at startup.
- On first Kobo download of each book, run `kepubify` via the [F0.5 worker](0-5-background-worker.md); cache output at `<data_dir>/kepub/<book_id>.kepub.epub`; serve that on subsequent requests.
- Invalidate cache on `book.last_modified` bump.
- If kepubify is absent, fall back to plain EPUB with a one-time admin warning in the log.
- Bundle kepubify in the Nix dev shell and in release images; keep it optional at runtime for users who build from source.
- See [assumption A6](0-0-summary.md#assumptions) on kepubify's stability.

## Technical considerations

- **Stream the sync response** via `axum::body::StreamBody` — **do not** copy Calibre-Web's `SYNC_ITEM_LIMIT=100` cap. See [calibre-inspection §3](../calibre-inspection/3-performance-pain-points.md) and [recommendation #13](../calibre-inspection/7-recommendations.md).
- `books.uuid` column needs an index — Kobo identifies books by uuid, not by integer id.
- Sync tokens stored in a `kobo_sync_tokens` table keyed on user.
- Reading state flows through [F2.1](2-1-progress-sync.md) internally — Kobo endpoints translate. "Reading state" here is **position + read status + statistics** only: the `/kobo/v1/library/<uuid>/state` PUT carries `CurrentBookmark` (the *position anchor* + progress percent — **not** a user-created bookmark), `Statistics`, and `StatusInfo`. The protocol exposes **no endpoint for user annotations** (highlights/notes/quotes); those never sync. Verified against [Calibre-Web `cps/kobo.py`](https://github.com/janeczku/calibre-web/blob/master/cps/kobo.py) (`HandleStateRequest`).

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) (uuid index).
- [F0.3 Auth](0-3-auth.md).
- [F0.5 Background worker](0-5-background-worker.md) (kepubify jobs).
- [F2.1 Progress sync service](2-1-progress-sync.md) (reading-state translation).

## Risks

- Kobo protocol is undocumented. Calibre-Web's implementation is our reference; scope for v1.0 is "parity with Calibre-Web minus the 100-item cap."
- kepubify absence degrades gracefully; not a blocker.
- **⚠️ Annotation data-loss hazard.** A Kobo expects an annotation channel when syncing (it has one against Kobo's own cloud). Pointing the device at a self-hosted server that doesn't speak that channel can make the Kobo **delete its own local highlights/notes** ([#2610](https://github.com/janeczku/calibre-web/issues/2610), [#1783](https://github.com/janeczku/calibre-web/issues/1783)). Mitigations: warn the user that first sync may clear on-device annotations, and recommend running the [F4.4 USB import](4-4-kobo-annotation-import.md) **before** the device's first wireless sync. Never claim to preserve device annotations we don't store.

## Deferred: wireless sync

Stage 1 (wired KEPUB sideload) shipped. The wireless `/kobo/v1/*` protocol above
is deferred to a later initiative. These design decisions are settled and should
be honored when it's picked up:

- **Scope: per-shelf opt-in — never whole-library.** Mark specific
  [F3.1 shelves](3-1-shelves.md) as "sync to Kobo"; only those books sync. A
  single-book shelf covers the "just one book" case. This matches Calibre-Web's
  sync-selected-shelves model.
- **Device auth token: persistent, re-viewable URL.** A per-user token in the URL
  path (`/kobo/<TOKEN>/v1/…`), which the user pastes as the Kobo's `api_endpoint`
  in `.kobo/Kobo/Kobo eReader.conf`. Stored re-viewably (Calibre-Web style) with a
  Regenerate action, scoped per-user and revocable. Table: `kobo_auth_tokens`.
  Sync-cursor state (`x-kobo-synctoken`) in `kobo_sync_tokens`, keyed on user.
- **Store endpoints: stub locally, never proxy** to `storeapi.kobo.com` — keeps
  the server self-hosted and private. `v1/initialization` is the one load-bearing
  non-stub (resources map pointing at this server).
- **Read status: new `book_read_state(user_id, book_uuid, read_status, percent)`
  sibling table** (there is no read-completion column today) to round-trip Kobo
  `StatusInfo`; position routes to `reading_progress` via `upsert_progress`.
- **Streaming sync, no 100-item cap** (`axum::body::Body::from_stream` over a
  keyset cursor).
- **⚠️ Data-loss warning UX is mandatory before exposing any device URL:** the
  settings card that shows the URL must render the "first sync may erase your
  Kobo's highlights" warning in the same component, and the docs must recommend
  the [F4.4 USB import](4-4-kobo-annotation-import.md) *before* first wireless
  sync.
- **Endpoint contract tests** replay the device sequence (`initialization →
  library/sync → download → PUT state`) at the HTTP layer (Playwright can't drive
  a Kobo), seeded from a golden fixture captured from a real device; DTOs ported
  field-for-field from a pinned Calibre-Web revision. A real-device smoke test
  gates advertising the URL.
- **Release labels:** the wireless PRs add migrations → tag each migration-bearing
  PR `minor version`.

---

[← Back to roadmap summary](0-0-summary.md)
