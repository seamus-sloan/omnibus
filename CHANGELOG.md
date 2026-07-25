# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Releases are cut automatically on merge to `main` (see
`.github/pull_request_template.md`): by default a merge cuts a patch
release, and the `minor version` / `patch version` / `no release`
labels override that default. Unlabeled PRs that only touch docs
(`*.md`, `docs/`) or CI (`.github/`) skip the release automatically.
Entries here are curated manually rather than generated from the raw
commit log.

## [Unreleased]

### Added

- Highlights, bookmarks, and journal entries now carry a `client_id` — an
  identity the creating device mints at the moment of the gesture (migration
  `0049`). Creates are idempotent on it, and `/api/highlights/{id}`,
  `/api/bookmarks/{id}`, and `/api/journals/{id}` accept it in place of the
  server's row id. This is what lets an annotation created and then edited or
  deleted while offline replay coherently: both ops name the same handle, where
  before the follow-up addressed a row id the server had not yet assigned and
  was silently dropped — leaving a highlight the reader had deleted still on the
  page. Rows predating the migration and clients that don't mint one are
  unaffected; the numeric id remains a valid handle
- `ProgressRecord` now carries `client_updated_at` — the reader's own clock for
  the position, which is what `upsert_progress` orders conflicts on. `updated_at`
  is a server arrival timestamp, so a client comparing its position against it
  was measuring the drift between two machines' idea of the time rather than
  which position was further along: a device running ahead of a self-hosted
  server suppressed every sync offer, one running behind raised them constantly.
  Absent against a pre-`0051` row, where callers fall back to `updated_at`
- iOS app: the whole library is mirrored on device, not just the page last
  shown. `LibraryIndex` pulls every book on sign-in, on foreground, and on
  reconnect, which is what lets paging, the format filters, and the "Offline"
  bucket work with no network — the last of those now means the whole library
  rather than whichever page happened to be loaded. A pass accumulates in a
  staging table one committed page at a time and only becomes the mirror once
  it finishes, so no write lock is ever held across a network call and an
  interrupted pass resumes from its stored cursor rather than starting over

### Changed

- iOS app: reads are now local-first. Every screen paints from the on-device
  replica immediately and updates underneath if the server answers differently,
  instead of blocking on a round trip. `Cache.live` replaces the old
  cache-first/network-first pair — a read that can answer twice, which is what
  the previous 45-second freshness window was standing in for
- iOS app: search answers from the device first. The local mirror matches on
  the keystroke and the server's richer, BM25-ranked answer (which knows
  authors, series, and tags as entities) replaces it when it lands. Search was
  the one surface that always waited on a round trip and returned nothing at
  all offline
- iOS app: the audiobook player reconciles its position with the server before
  playing, the way the reader already did. It resumed from the local replica
  alone, and the periodic position write — stamped with a fresh clock, which
  the server orders conflicts on — then overwrote whatever another device had
  reached, within a second of pressing play. Listening position now survives a
  handoff in both directions, and a position that lands after the open is
  offered rather than applied
- iOS app: opening an audiobook no longer resets its saved playback speed.
  `rate` is singleton state that assigning the newly-loaded book's speed also
  wrote *back*, so opening a second book echoed the first book's value — and
  offline, where the read falls back to 1.0, that echo queued a write that
  reset a speed set on another device
- iOS app: a book taken offline now brings its position, highlights, bookmarks,
  journal entries, read status, and (for audiobooks) its manifest down with the
  file. The replica only holds what a screen has already asked for, so a book
  downloaded from the shelf and never opened on the device opened at the
  beginning with nothing marked in it — the file was the one part that had
  never been the problem
- iOS app: signing in as a second account on the same device no longer leaves
  the previous user's shelves on the Library landing rail. The wipe list is
  matched by prefix and `shelves` / `shelf:` / `shelf_page:` do not cover
  `shelf_previews`; a test now pins every user-scoped cache key against it
- iOS app: a replica read can no longer discard a write made while it was in
  flight. The guard that holds off adopting the server's answer ran only before
  the request, so a highlight made during a slow round trip was overwritten by
  the answer that predated it
- iOS app: a completed library sync only prunes downloads for formats the sync
  actually covered. `GET /api/ebooks` lists whatever the server's library-path
  settings currently name, so an unset audiobook path made every audiobook look
  deleted and took every downloaded audiobook file with it
- iOS app: marking a book finished offline holds off the stats summary it feeds,
  which only the journal side of the same completion signal used to do
- iOS app: a rejected create no longer reports one refused change several times
  over — the queued ops that address what it would have made are retired
  against the same cause instead of replayed into a 404 apiece
- iOS app: the reader opens at the synced position instead of offering it.
  On a cold open there is nothing to move out from under the reader, so the
  position settles behind the loading state — Whispersync's own behaviour. The
  banner is now reserved for a position that arrives *mid-read*, where jumping
  the page really would be worse than being behind
- iOS app: syncs at the moments Whispersync does — app forward, app away, book
  open, book close, and on a background refresh while the app isn't running —
  rather than only on a cold launch, a sign-in, or a reconnect it happened to
  notice. Backgrounding flushes the open book's position and drains the outbox
  inside a background-task assertion, so a position written on the way out
  isn't a race against suspension. The background pass is deliberately narrow
  (drain the outbox, refresh the resume rail) because the system grants seconds
  and overrunning costs future grants; the library mirror waits for a
  foreground

### Fixed

- iOS app: saving a Kindle email with no connection now reports the failure
  instead of claiming success. The button swallowed the error and flipped to
  "Saved" regardless, so it confirmed a change that existed nowhere — not on the
  server, and not in the outbox, which by policy does not carry account
  configuration
- Reading positions are now ordered by the *reader's* clock rather than by when
  the write reached the server. `POST /api/progress` accepts an optional
  `client_updated_at` (migration `0051`) and keeps whichever position carries
  the later one, answering with the winner. Ordering by arrival meant a position
  read offline at 10:00 and pushed at 18:00 overwrote one read on another device
  at 14:00 — and then stamped itself as the newest thing on the server, so no
  client could tell. Clients that send no clock are unaffected: the server
  stamps its own and the behaviour reduces to the previous last-write-wins
- `GET /api/shelves/containing/{uuid}` answers "which hand-picked shelves hold
  this book" in one request. The iOS book screen had to fetch every visible
  shelf's page and scan it — one request per shelf, on every book opened
- iOS app: an offline write is now recorded before it is attempted, not after it
  fails. The old order left a window the length of the request in which the
  replica already showed the change and nothing had recorded it, so a suspend or
  a crash there lost the write outright while the UI went on showing it until
  the next revalidation quietly replaced it. It also let a write racing ahead of
  the queue reach the server before ops it depended on
- iOS app: the Continue-reading rail now moves while offline. It is a server
  read, and the outbox correctly holds off revalidating it while a position
  write is queued — but nothing wrote the local side, so a session read offline
  left the rail showing where you were beforehand, and a book started offline
  never appeared on it at all
- iOS app: queued writes are retried on a timer while the server is reachable.
  Every other trigger is an event — a reconnect, a foreground, a book opened —
  and none of them fires for a server that answers 5xx: the radio never changes,
  so the reachability probe never arms and the app simply stops trying
- iOS app: a replayed op is retired the moment it lands, instead of the whole
  batch being retired at the end of the pass, and the background-task expiry
  handler now cancels the drain rather than only ending the assertion. A drain
  frozen mid-pass by suspension left landed writes still queued, and re-sending
  a delete 404s — surfacing as a "couldn't sync" warning for a change that had
  in fact synced
- iOS app: `408`, `425`, and `429` are no longer treated as permanent
  rejections. Every 4xx was terminal, so a self-hosted server behind a proxy
  that rate-limits discarded queued positions and highlights that would have
  landed a minute later
- iOS app: one op failing with a 5xx no longer holds back every unrelated write
  behind it. Order within a kind is load-bearing — a note addresses the
  highlight created ahead of it — but order across kinds is not, so a stalled
  kind is now skipped for the rest of the pass instead of stopping it
- iOS app: adding a book to a shelf offline now shows on the shelf. The remove
  path spliced the cached page; the add path only ticked the checkmark, so the
  book was checked on its own screen and absent from the shelf. Both now keep
  the shelf's book count in step too
- iOS app: an open book reports its reading session every few minutes rather
  than only when the book closes or the app backgrounds, so an afternoon's
  reading in one sitting isn't lost with the process
- iOS app: a listening report's time span is derived from the seconds actually
  listened. A window holding less than five seconds is deliberately left open to
  accumulate, so its opening timestamp drifted arbitrarily far back behind a few
  paused hours and the report landed in the wrong day
- iOS app: downloads for books the library no longer has are removed after a
  completed mirror sync. A book deleted on the server left its file on disk
  forever — gone from the "Downloaded" filter, which reads through the mirror,
  while still counted under "Storage used"
- iOS app: the Downloads list falls back to the library mirror for titles, so a
  book downloaded from the grid without opening its detail screen no longer
  renders untitled
- iOS app: a server answer that arrives as its screen is dismissed is written to
  the replica instead of discarded. SwiftUI tears these reads down on every
  navigation, so the app was throwing away data it had already fetched
- iOS app: reachability is seeded from the network path already known at launch
  rather than assuming online until the first update lands — which was the
  window the library screen asks its first question in
- iOS app: a cancelled request was recorded as evidence the server was
  unreachable, so the app flipped to "Offline" and stayed there for a probe
  interval after any moment that cancels reads in bulk — pulling to refresh,
  which restarts every read still in flight, or simply scrolling the grid, which
  cancels a cover fetch per cell that leaves the screen. Cancellation now
  unwinds without touching the reachability state or the back-off
- iOS app: opening a book that isn't downloaded while the server is unreachable
  now says so on the tap, instead of showing a loading spinner that never
  resolved. The reader's failure path relied on epub.js reporting a load error
  back, which for a book whose bytes never arrive it never did — so the overlay
  stayed up indefinitely. Read and Listen are now refused up front when the file
  isn't on the device and the server can't be reached, and a fetch that fails
  after the reader has already opened marks it failed directly rather than
  waiting to be told
- iOS app: the book detail hero showed the generated plate instead of the cover
  art whenever the server was unreachable. Each thumbnail size is a separate
  file under a separate cache key, and the hero asks for a larger one than the
  grid — so a book browsed online had the grid's size cached and the hero's not.
  A cover now falls back to any other cached size of the same picture, which
  also puts art on screen a beat sooner when the exact size is still in flight
- iOS app: a book's detail page failed with "Could not connect to the server"
  whenever the server was unreachable and that book's page had not been opened
  before — the replica holds only what a screen has already asked for, and the
  read had no other source. It now falls back to the local library mirror, which
  holds the same `Book` the detail endpoint returns, so every book in the
  library opens offline rather than only the ones already visited. The library
  grid has fallen back this way since it was written; the detail page never did
- iOS app: every `0` and `1` in a queued request body replayed as `false` /
  `true`. `JSONSerialization` returns `NSNumber` for both booleans and numbers,
  and the re-encoder's boolean test was vacuously true, so the server rejected
  the replay as a type error and the outbox marked it unretryable — losing a
  one-star rating, a 1.0x playback speed, or a journal entry at 0% made while
  offline, with only a refused-writes count to show for it
- iOS app: listening sessions were never recorded. The only code path that
  reported one hung off a player `close()` that nothing called — the mini bar
  had no dismiss control — so audiobooks contributed nothing to total time, the
  streak calendar, or the listening trend. Listening is now checkpointed
  wherever it definitely pauses (a pause, a backgrounding, a book switch, the
  end of the book), and the mini bar has a dismiss control
- iOS app: a reading session reported wall-clock time from opening a book to
  closing it, so a book left open overnight counted as eight hours read. The
  reader now stops counting when the app leaves and starts again when it
  returns, reporting each stretch separately
- iOS app: one queued write held *every* replica read in the app on its cached
  answer, not just the rows it was about — and a write the server kept
  answering 5xx for was retried forever, so a single stuck op froze all fresh
  data indefinitely. Revalidation is now scoped to the keys the queue actually
  describes, and an op that cannot land is held aside after six attempts
- iOS app: deleting a shelf while offline dropped the cached shelf list without
  anything to refetch it from, so the Shelves tab rendered its "no shelves"
  empty state until the device reconnected; pulling to refresh offline did the
  same. Deletes now splice the shelf out of the cached lists, and a read that
  can neither answer from the replica nor reach the server reports a failure
  instead of an empty result
- iOS app: signing out abandoned anything still queued — every op 401s once the
  token is revoked, and the writes were wiped outright if a different user
  signed in. Sign-out now pushes what it can first
- iOS app: the library mirror's re-sync throttle was held in memory, so it never
  applied to the one sync guaranteed to happen and every cold launch re-paged
  the entire library. It is now persisted. A pass that stopped at the 50,000-book
  ceiling also promoted itself as complete, which deleted every book past the
  bound from the mirror; it is now treated as an interruption
- iOS app: opening a book issued two identical position reads, and the deadline
  bounding the first was not enforceable — cancelling it unwound through the
  outbox drain it triggered, which cannot be cancelled. There is now one read,
  waited on briefly and then left to run: it sets the opening position if it
  settles in time and offers a jump if it lands after
- Session reports are idempotent on a client-minted `client_id` (migration
  `0050`), so a report whose reply was lost no longer double-counts reading time
  when the outbox replays it. Web clients post once and never retry, so they
  send no handle and are unaffected
- iOS app: a book's reading position and its listening position shared one
  replica slot and one outbox coalescing key, so on a dual-format book the
  reader and the player overwrote each other's place — and a listening position
  queued offline deleted the reading position queued behind it. Both are now
  keyed on `(book, format)`, matching the web client, and the position read
  finally sends `?format=`, which the server had been defaulting to `epub`
- iOS app: a server read taken while writes were still queued replaced the
  replica with a list that predated them, so an annotation made offline could
  disappear the moment anything read it back. Reads now drain the outbox first
  and hold off while a queued write still describes the rows being read — the
  same guard the web client applies in `frontend/src/offline/cache.rs`, scoped
  per key rather than to the queue as a whole
- iOS app: downloads ran on a foreground `URLSession`, so backgrounding the app
  part-way through an audiobook lost the whole transfer. They now run on a
  background session that continues while the app is suspended and reports back
  even if the app was terminated
- iOS app: the download registry was keyed on the book alone, so a dual-format
  book could hold only one of its two files — always the ebook — and the player
  was handed that epub as though it were the audiobook. It is now keyed on
  `(book, format)` and the book screen offers each format separately
- iOS app: the Continue carousel never showed more than one book. It reads
  `/api/progress/recent`, whose `limit` defaults to 1 server-side, and never
  sent one
- iOS app: a write the server rejected outright was deleted silently while the
  replica went on showing the change. Rejected ops are now held out of the queue
  and surfaced on the You tab with a way to discard them
- iOS app: pointing at a different server kept the previous one's library
  mirror, cached pages, and download badges — the account-switch wipe keys on
  the username, which says nothing about which server issued it. Repointing now
  clears everything local
- iOS app: adding or removing a book on a shelf while offline left the shelf
  picker showing the old membership, inviting a second tap that queued a
  duplicate
- iOS app: an annotation made offline vanished when the book was closed. The
  highlight, bookmark, and journal write paths queued the op but never wrote the
  entry to the replica, so it survived only as long as the view holding it. All
  three now write optimistically before the request and reconcile against the
  server's list afterwards
- iOS app: opening a downloaded audiobook offline silently reset the playback
  speed to 1.0×. The rate was read straight from the network with a `1.0`
  fallback while the *write* path queued correctly; it now reads through the
  replica and writes through on change
- iOS app: the shelves a book belongs to came back empty offline, so every
  shelf it was already on read as unchecked and tapping one queued an add the
  server would reject as a duplicate. That read is now cached — a remembered
  answer is right far more often than a blank one, and the live read corrects it
- iOS app: deleting a shelf offline failed outright instead of queueing. It has
  a real server id to name, so it now replays like any other write. Creating a
  shelf still requires the server, since everything keyed on the new shelf's id
  would otherwise queue against an id that doesn't exist yet
- iOS app: entering a server address while a session was already in the
  keychain — a reinstall, or re-entering an address after "use a different
  server" — landed on the library without confirming the identity, draining the
  outbox, or mirroring the library. The app would run indefinitely with an empty
  mirror and no offline search until something else happened to trigger a sync.
  That entrance now does the same post-authentication work as a cold launch and
  a sign-in

- iOS app: books downloaded for offline use stopped opening after an app
  update or reinstall, silently falling back to streaming and so failing
  entirely with no network. The download registry stored an absolute path
  containing the app's data-container UUID, which iOS reassigns on reinstall,
  so every stored path went stale even though the files were still on disk.
  Paths are now container-relative and existing rows are migrated on launch
- iOS app: covers never appeared offline. The disk cache keyed filenames on
  `String.hashValue`, which Swift seeds randomly per process, so nothing
  written on one launch was findable on the next and the cache missed on every
  cold start. Keys are now SHA256. Downloading a book also pulls its artwork
  down while the server is still reachable
- iOS app: going offline mid-session made the app crawl — opening a book cost
  three sequential request timeouts before the reader was even built, and every
  screen re-paid the timeout because nothing remembered the server was
  unreachable. The client now fails fast behind a single-probe back-off, and a
  server that comes back is picked up within ten seconds by a health poll
  rather than waiting for the user to hit a screen that happens to fetch
- iOS app: a self-hosted server reachable only over plain http could not be
  connected to at all — entering its address failed with "the App Transport
  Security policy requires the use of a secure connection". The Info.plist set
  `NSAllowsArbitraryLoads` alongside `NSAllowsLocalNetworking`, and iOS 10 and
  later *ignore* the former whenever the latter is present, so ATS stayed fully
  enforced for every address that wasn't link-local. A server on the same LAN
  worked, which is what hid it
- iOS reader: the highlight menu is no longer fiddly. Tapping a highlight also
  registered as a tap on the page, so the bars flipped underneath the menu as
  it opened and closing it left them wrong until another tap put them back;
  and highlighting a passage left it visibly selected, which spent the next tap
  dismissing the selection instead of doing what you asked. One tap anywhere
  now closes the menu — including in the page gutters, which used to turn the
  page out from under it

### Added

- iOS reader: Apple Books-style chrome. The reading view is now a close button,
  a menu button, and two centred labels — "N pages left in chapter" and the
  page count — instead of filled bars framing the page. Everything else sits
  behind the one menu: Contents, Bookmarks & Highlights, Themes & Settings, and
  a strip of quick actions. The Contents row doubles as a scrubber — press and
  hold, then slide, with the chapter and page you'd land on named above it — so
  seeking to any point in a book no longer means opening the contents and
  picking a chapter, and a jump leaves a return button holding the page you
  left. Contents, bookmarks, and highlights share one segmented sheet instead of
  being split across separate chrome buttons with highlights unreachable
  entirely, and the contents list now carries a page number per chapter
- Reader: highlights on the page can be tapped, opening a menu anchored to the
  passage — recolour, note, copy, share, remove. The server, the REST API, and
  the Swift service already supported notes, recolouring, and deletion; the
  reader could only ever *create* a highlight, so every one of those was
  unreachable. Notes are written against the quoted passage and mark the
  passage on the page, so an annotated line is findable without remembering it
- iOS app: books can be added to a shelf from inside the shelf — search or
  browse the library, select any number, add them in one call. Filling a shelf
  previously meant leaving it and adding books one at a time from each book's
  own detail screen
- iOS app: journal entries can be edited. The server has supported
  `PATCH /api/journals/{id}` all along, but the app only created and deleted —
  so "Save as draft" was a one-way trip with no way to finish or publish it

### Fixed

- Reader: chapter navigation no longer twitches. `displaySettled` lands a
  first pass, waits for fonts and injected CSS, then re-displays to correct the
  landing — and that correction was visible on every TOC jump. It now happens
  behind a faded stage, the same settled-reveal the book-open path already used.
  Gated on `allowScriptedContent`, since without scripts in the section iframe
  `fonts.ready` never settles and the correction would only land on a 1.5s
  fail-safe — too long to hold a blank stage, so those builds keep the old
  behaviour
- iOS app: tapping the Search tab while already on it now returns to the plain
  search page, unwinding any pushed authors/series/tags screens and clearing
  the query. The tab bar previously swallowed a tap on the current tab, so the
  standard iOS "return to this tab's root" gesture did nothing
- iOS app: saving metadata overrides failed for every book. The client sent
  `creators` as an array of plain strings, but the wire type is
  `Vec<Contributor>` (objects), so `Json<MetadataOverrides>` extraction
  rejected the request before it reached validation — and since the Authors
  field is always prefilled from the book, no edit could ever be saved
- iOS app: the metadata editor submitted every field on every save, writing
  overrides for fields nobody touched and pinning scanned values against
  future rescans. It now sends only what actually changed
- iOS app: metadata fields used their placeholder as their only label, so a
  filled row lost its name — a bare "en" with nothing to say it meant Language

### Changed

- iOS app: gave the native app the Atrium identity it was missing — an
  editorial masthead (drawn mark, italic serif wordmark, accent rule) on the
  library, the serif display face on every navigation title, a named motion
  vocabulary (`Motion.lift/settle/snap/glide/page`) replacing scattered spring
  literals, cover-to-detail zoom navigation, staggered grid entrances, lit
  covers with a gutter shadow, a book-tipping press, and an ambient wash that
  takes the colour of whatever you're currently reading
- iOS app: rebuilt the book detail screen as a jacket rather than a record —
  the accent halo now sits behind the artwork instead of peaking under the
  navigation bar, the jacket dissolves and parallaxes on scroll, the bar title
  appears only once the real one leaves, and rating / status / shelves /
  offline collapsed from four floating controls into one hairline-divided
  plate with a custom status selector. Details read as a colophon (mono
  small-caps keys) and the description is set in the reading face
- iOS app: book detail now leads with About — the blurb set as prose with a
  raised initial and an expand control — followed by tags, then a rating band,
  status, and shelving. The page is paced for reading about a book rather than
  packed for reach, since anyone on this screen has chosen not to be reading
- iOS app: journal entries render their markdown instead of showing the raw
  source, and are drawn as marginal notes against a rule — accent for your own,
  amber for drafts — in the reading face, with long entries collapsed behind a
  More control. The empty state invites writing rather than reporting "No
  entries yet"
- iOS app: "In your library" and "Details" dropped their filled panels for
  hairline records on the page ground, matching the sections above them; the
  short colophon no longer hides behind a disclosure chevron
- iOS app: ratings are set by dragging across the stars, in half-star steps,
  with the value tracking your finger. A tap resolves through the same mapping,
  so the left half of the third star is 2.5 — previously half stars needed a
  second tap on a star you had already chosen
- iOS app: brought the shelves flow onto the shared design language — the shelf
  header leads with the accent rule and a mono meta line, the "add to shelf"
  picker and the shelf composer dropped their stock `List`/`Form` chrome for
  the app's plates, records, and pill selectors, and an empty manual shelf now
  invites filling instead of pointing elsewhere. The plate vocabulary moved out
  of the book-detail feature into `Design/Components/Plate.swift`, since the
  metadata editor and shelves both build on it
- iOS app: rebuilt the metadata editor as an editable colophon matching the
  detail screen's Details plate — persistent mono small-caps labels, per-field
  accent markers showing what you've changed, a Save that stays disabled until
  something differs, and authors as removable chips with an add field instead
  of one comma-delimited string

## [v0.11.4] - 2026-07-24

### Changed

- Added missing row `LIMIT` to `list_wishlist` (#1259)
- Added test coverage for `ScanError` `Lookup`/`Physical`/`Sqlx` variants (#1263)

## [v0.11.3] - 2026-07-24

### Changed

- Added test coverage for `server::backend::covers` (#1262)

## [v0.11.2] - 2026-07-24

### Fixed

- Fixed `deny.toml` `skip-tree` comment for `rustc-hash` (#1276)

## [v0.11.1] - 2026-07-24

### Changed

- Enforced `-D warnings` and wasm32 clippy in `just lint` (#1270)

## [v0.11.0] - 2026-07-23

### Added

- Added the Google Books API key as a first-class Settings field, used as a
  fallback rung in the check-in ISBN lookup ladder (#1292)
