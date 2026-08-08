# Kobo wireless sync — real-device smoke test

The HTTP-layer contract tests in
[`server/src/backend/kobo/tests.rs`](../server/src/backend/kobo/tests.rs) pin
this server's side of the wireless protocol against a **synthetic** golden
fixture — no physical Kobo capture is available in this repo's dev/CI
environment (see that file's `full_device_sequence_replays_initialization_through_state_put`
for the fixture and its documentation). They cannot prove the device's own
firmware parses what Omnibus sends, accepts the shapes Omnibus expects, or
that Omnibus's read of a real device's traffic matches what the device
actually transmits.

This checklist is the **manual gate** between "the contract tests pass" and
"safe to tell a user to point their Kobo's `api_endpoint` at this server" (see
[docs/kobo.md § About wireless sync](kobo.md#about-wireless-sync-experimental)).
Run it against a real Kobo — not an emulator — before:

- Advertising wireless sync as non-experimental in the UI or docs.
- Merging a change that touches `server/src/backend/kobo/`, `db/src/kobo*`,
  or `db/src/kobo_position*`.
- Cutting a release whose notes mention Kobo sync.

> [!WARNING]
> Back up the device first. `.kobo/KoboReader.sqlite` over USB, or one of the
> tools listed in [docs/kobo.md](kobo.md#about-wireless-sync-experimental). A
> bad sync can wipe on-device highlights, notes, and progress.

## Setup

1. A test Kobo (any model with wireless sync — firmware version matters less
   than device family; note the exact firmware version in the result).
2. An Omnibus instance reachable from the device's Wi-Fi network (LAN IP or
   tunnel — not `localhost`).
3. A test user account with a shelf containing 2–3 books: at least one EPUB
   and, if available, one CBZ-only comic.
4. `Account → Kobo wireless sync → Add a Kobo`, then point the device's
   `api_endpoint` at the printed URL per
   [docs/kobo.md § Setting up wireless sync](kobo.md#setting-up-wireless-sync).

## Checklist

Check off each item; note the firmware version and device model at the top of
the result, and file a GitHub issue for anything that fails (tag it
`kobo-sync`) rather than silently working around it.

- [ ] **Handshake.** `Sync now` on the device completes without an error
      dialog or an endless spinner. (Exercises `initialization` +
      `auth/device`.)
- [ ] **Library sync — first pull.** Every opted-in book appears in the
      device's library list with the right title and author.
- [ ] **Download.** Each book opens and renders; page turns work. For an
      EPUB, confirm it downloaded as a `.kepub.epub` (Kobo's book-info screen
      shows this) — if it silently fell back to plain EPUB, check
      `OMNIBUS_KEPUBIFY_PATH` server-side, not the device.
- [ ] **Read-status push (device → server).** Open a book, read a page or
      two, back out. In Omnibus's web UI, confirm the book's read status
      moved to "Reading" and the Continue-reading rail shows it.
- [ ] **Position push (device → server).** Read further, note the on-device
      percent, then check the position shown in the Omnibus web reader for
      the same book roughly matches (exact percent parity isn't
      guaranteed — chapter-relative vs whole-book percent is one of the
      open questions `kobo/dto.rs` documents).
- [ ] **Position pull (server → device).** In the Omnibus web reader, jump to
      a new position in a book the device already has. Trigger a device
      sync. Reopen the book on the device — it should resume near the new
      web position (this is the CFI→KoboSpan derivation path).
- [ ] **Finished status round-trip.** Mark a book "Finished" on the device
      (or read to the last page). After a sync, Omnibus's web UI shows it
      finished; conversely, marking Finished in the web UI and syncing
      should update the device's status.
- [ ] **Web-created highlight reaches the device.** Highlight a passage in
      the Omnibus web reader for a book the device has downloaded. Sync the
      device, open the book, confirm the highlight appears **at the right
      location**.
- [ ] **Highlight colour.** Create highlights in more than one colour from
      the web reader, sync, and check each lands on the device in the
      *matching* colour — not all defaulting to yellow. This is the specific
      regression `docs/kobo.md`'s "Highlight colour on-wire shape" note
      describes as still unconfirmed; a capture from this step (device
      firmware version + which colours landed correctly) is exactly what
      that note asks for.
- [ ] **Device-created highlight reaches the web reader.** Highlight a
      passage on the device, sync, and confirm it appears in the Omnibus web
      reader at the right location.
- [ ] **Removal.** Remove a book from the opted-in shelf (or un-flag the
      shelf's "Sync to Kobo"). After a sync, the book is archived/removed
      from the device's library — and its own annotations/progress on the
      device are not corrupted by the removal.
- [ ] **Second sync is quiet.** With nothing changed since the last sync, a
      further "Sync now" completes quickly with no re-downloads and no
      duplicate library entries.
- [ ] **CBZ-only book** (if the library has one): downloads and opens as a
      comic, not attempted as an EPUB/KEPUB conversion.

## Recording a result

Append a dated entry (device model, firmware version, pass/fail per item, and
a link to any filed issue) to this file's bottom, or link a GitHub issue
tagged `kobo-sync` that captures the same information — whichever this repo
is doing at the time. As of this writing no run has been recorded yet; the
first real-device pass should replace this line with either a result summary
or a pointer to the tracking issue.
