# 08 — What may be queued offline

Both clients hold a durable mutation outbox — `OpKind` in
[omnibus-ios/omnibus/Offline/SyncEngine.swift](../../omnibus-ios/omnibus/Offline/SyncEngine.swift)
and the `Op` enum in
[frontend/src/offline/outbox.rs](../../frontend/src/offline/outbox.rs) — that
records a write locally, applies it optimistically to the replica, and replays
it when the server is reachable again.

**The outbox carries per-user content state. It never carries configuration,
and it never carries commands.**

A write may be queued only if it passes **all four** tests below. Any one of
them failing is a veto.

## 1. Content state, not configuration

Three tiers, and only the last one is queueable:

- **Instance configuration** — library paths, API keys (Hardcover, Google
  Books), SMTP, metadata overrides, anything under `/api/settings`. Never
  queued. Other actors — a second admin, the indexer, the filesystem — move
  underneath a deferred write, and last-write-wins on a config row means a
  phone that was offline for a week silently repoints the library.
- **Account configuration** — the per-user settings that aren't content, today
  just `kindle_email`. Never queued. It is per-user, but it is still
  configuration: set rarely and deliberately, essentially always with a
  connection, and it addresses where a *server-side action* delivers. A stale
  replay redirects a delivery.
- **Content state** — where you are in a book, what you marked in it, what you
  wrote about it, how you rated it, which shelves hold it. Queueable, subject
  to the remaining tests.

The instinct to check: *would a second person, or the server itself, plausibly
have changed this while the device was away?* If yes, it is configuration.

## 2. An assertion, not a command

A queued write must mean the same thing whenever it lands. "This is the value"
survives deferral; "do this now" does not. A reindex queued at 09:00 and
replayed at 18:00 is a job nobody asked for, and `POST /api/kindle/send` queued
on a plane delivers a book the reader has since finished.

Excluded by this test: reindex, library scan, FTS rebuild, send-to-Kindle,
send-to-Kobo — every route whose value *is* the act happening now.

## 3. Nameable and complete offline

The device must be able to name what it is changing and fill in the whole
payload without asking the server.

- **Nameable.** Client-minted `client_id`s are what make this true for
  annotations (migration `0051`): a highlight created offline and edited
  moments later has both ops naming the same handle. A shelf create has no such
  handle, so the iOS client refuses it rather than queueing every later op
  against an id that does not exist — see `createShelf` in
  [omnibus-ios/omnibus/Services/UserDataService.swift](../../omnibus-ios/omnibus/Services/UserDataService.swift).
  The web client solves the same test differently, with negative temp ids that
  remap on drain; both are valid answers to it.
- **Complete.** The check-in routes fail here. `bookUUID` and `ExternalBookMeta`
  come out of the server's ISBN lookup ladder, so queueing a check-in queues a
  question rather than an answer.

## 4. Truthfully representable in the replica

Every queued write is applied optimistically, so the client must be able to
render the post-write state locally. If it can't, the UI shows a promise
dressed as a state. `createJournal` is the honest version of the edge case: it
shows the markdown source until the server has rendered it, rather than
guessing with a second renderer that would disagree.

A new queueable kind must also declare its blast radius in `OutboxScope`
([omnibus-ios/omnibus/Offline/Cache.swift](../../omnibus-ios/omnibus/Offline/Cache.swift)) —
which cache keys the server's answer would now predate. An undeclared kind
blocks every read, deliberately: over-cautious beats a stale answer silently
overwriting a queued write.

## The corollary

A write that fails any test must fail **visibly and immediately**. Never
`try?`-swallowed, never optimistically applied, never reported as success.
Where it is cheap, disable the control while `Connectivity.isOnline` is false
rather than letting the request fail after the fact.

## Current inventory

Queued (iOS `OpKind`):

| Kind | Writes | Coalesced |
|---|---|---|
| `progress:<uuid>:<format>` | reading / listening position | yes |
| `playback_rate:<uuid>` | audiobook speed | yes |
| `rating:<uuid>` | set / clear rating | yes |
| `read_status:<uuid>` | want / reading / finished | yes |
| `session` | session reports | no |
| `highlight` | create, colour, note, delete | no |
| `bookmark` | create, delete | no |
| `journal` | create, update, delete | no |
| `shelf_membership` | shelf delete, add books, remove book | no |

Not queued, by test:

| Write | Fails |
|---|---|
| `POST /api/settings`, API keys, SMTP | 1 — instance configuration |
| `POST /api/account/kindle-email` | 1 — account configuration |
| Metadata overrides | 1 — library-wide, every user sees it |
| Book uploads | 1 — library-wide (and a GB-scale body has no business in `ops`) |
| Reindex, scan, FTS rebuild | 2 — commands |
| Send to Kindle / Kobo | 2 — commands |
| Shelf create (iOS) | 3 — no client-minted handle |
| Check-in, physical-only, wishlist | 3 — payload comes from the server's lookup |
| `POST /api/shelves/preview` | not a mutation; a read wearing POST |

## Adding a write path

1. Run the four tests. If any fails, call `APIClient` directly and surface the
   error — do not reach for `SyncEngine`.
2. If it passes, add the kind to `OpKind`, declare its keys in `OutboxScope`,
   and write the optimistic replica patch.
3. Choose `coalesce` by whether the write is a repeated statement of one value
   (a position, a rate) or a discrete event (a highlight, a session report).
   Discrete events must not coalesce.
4. Cover it per [03-unit-testing.md](03-unit-testing.md) — the iOS suite's
   equivalent is `omnibus-ios/omnibusTests/OfflineSyncTests.swift`.

## Known divergence

`Op::SetKindleEmail` in `frontend/src/offline/outbox.rs` queues a write that
test 1 excludes; the iOS client does not. The two clients should agree, and the
web side is the one out of step.
