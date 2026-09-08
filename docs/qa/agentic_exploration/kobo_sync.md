# The Kobo scenario

A runner-driven scenario, like the iOS lane's offline outbox: it is **not** in
the weighted catalog, one web agent is handed it when the run is started with
`--kobo`, and the runner does half the work. A Kobo is a device, and no agent
has one — so the runner speaks the device's protocol with `curl` while the
agent watches the app.

Two halves, in this order: the **shelf** half proves a shelf marked for Kobo
sync reaches the device; the **sync** half proves a position written by the
device reaches the browser, and a position written in the browser reaches
the device.

## What the runner does before handing this over

Nothing. The agent sets the device up through the app, because that is what a
person does.

## Part 1 — register a device and sync a shelf (agent)

1. Go to your account page and find the **Kobo** section. Click **Add a
   Kobo**, give it a name a person would give a device — *Kitchen Kobo*,
   *Clara's Libra* — and journal the name. **Do not journal the endpoint
   URL.** It carries the device token, which is a credential, and the journal
   is kept forever; hand the URL to the runner out of band instead.
2. Read the setup steps the card shows. They are the instructions a real
   reader follows; if they do not make sense to you, that is a finding about
   the copy.
3. Create a **Smart** shelf with a rule that matches three or four books — an
   author, a genre — and confirm it fills. The web cannot fill a hand-picked
   shelf, and a Kobo can only be sent a shelf with books on it.
4. Open **Edit shelf** on it and tick the Kobo sync opt-in. Save. Reload and
   confirm the opt-in stuck. A system shelf (the wishlist) must not offer the
   opt-in at all; check that once.
5. Journal `shelf.create` and `shelf.edit` with the name and the opt-in, then
   tell the runner the shelf is ready.

## Part 2 — the device syncs (runner)

With the endpoint URL from the agent, `$KOBO` below:

```bash
curl -sS "$KOBO/v1/library/sync" | python3 -m json.tool | head -80
```

The first sync returns every book on the opted-in shelf as a `NewEntitlement`,
each carrying a title, a download URL, and a `ReadingState`. Check three
things and journal them under the agent's actor with `surface` `kobo`:

- every book on the shelf is present, and **no book off it** is;
- each entitlement's title and author are the book's;
- the response ends and does not repeat forever — a header `x-kobo-sync:
  continue` means more pages, and a second call must eventually return
  none.

Then, on one of those books, write a position the way the device does:

```bash
curl -sS -X PUT "$KOBO/v1/library/<uuid>/state" \
  -H 'Content-Type: application/json' \
  -d '{"ReadingStates":[{"StatusInfo":{"Status":"Reading"},
       "CurrentBookmark":{"ProgressPercent":42,"LastModified":"<now, RFC 3339>"}}]}'
```

Every sub-result in the response must be `Success`. Journal `progress.set`
under the agent's actor with the uuid, the percent, and `surface` `kobo`,
then hand the agent the uuid and the percent.

## Part 3 — the browser sees the device's position (agent)

1. Open the book's detail page. Its progress should read close to the percent
   the runner wrote, and its read status **Reading**. Reload once if not.
2. Check the continue surface on the home page shows the book.
3. Open the reader. It should open near that percent — within a page, since a
   percent is a page-grained position. Journal where it opened.
4. Read forward a chapter or so, then leave. Journal the position you left at
   and tell the runner.

## Part 4 — the device sees the browser's position (runner)

```bash
curl -sS "$KOBO/v1/library/<uuid>/state" | python3 -m json.tool
```

The `CurrentBookmark` must now carry a percent **ahead** of the one written in
Part 2, and a `Location` — the device-side anchor derived from the browser's
position. Journal what came back. Then run the sync once more: the book
should come back as a `ChangedReadingState`, not as a new entitlement.

Finally, ask the agent to untick the Kobo opt-in on the shelf and sync once
more: each book should return as a `ChangedEntitlement` marked removed.

## Journal

Agent: `shelf.create`, `shelf.edit`, `book.open`, `reader.progress` as their
flows prescribe. Runner: `progress.set` for the device's write and
`sync.check` for each sync, both under the agent's actor with `surface`
`kobo`. The audit checks the `progress.set` as a saved reading position and
reads `sync.check` as a look, so it neither checks nor flags it; the runner's
journal entries are the record. `shelf.edit` is checked as the shelf still
existing under the name it carries; the opt-in itself is not audited.

## Pass

- The shelf's books, and only those, reach the device.
- The device's position and status appear in the browser.
- The browser's position reaches the device, ahead of the device's own.
- Removing the opt-in removes the books from the device on the next sync.
- The wishlist never offers the opt-in.

## Fail

- A book not on the shelf is sent to the device, or a book on it is missing.
- The device's write is accepted but the browser shows the old position.
- The browser's later position never reaches the device, or reaches it
  behind the device's own.
- A sync that never terminates.
- The device's stale position overwrites a newer browser position. High
  severity.

## Sharp edges

- **Newest wins by event time, not arrival time.** The `LastModified` on the
  device's write is what the server arbitrates against. A device write with a
  timestamp older than the browser's last position is *meant* to lose.
- A percent from a device is whole-book and page-grained. The reader opening
  at the top of the right page is agreement.
- The shelf half sends only opted-in shelves. A book in the library but on no
  synced shelf is correctly absent from the device.
- The device token is per device and persists. Remove the device from the
  account page at the end of the scenario, and journal that you did.
