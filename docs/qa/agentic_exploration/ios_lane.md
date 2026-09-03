# The iOS lane

You are the run's **one** iOS agent, using the native app on a simulator
against the same instance every other agent is using. Read
[start.md](start.md) first — everything in it applies. This file only says
what is different about your surface, and hands you the one scenario that
exists solely because your surface can run it.

There is never more than one of you. Two agents on one simulator would share a
keychain, a container and a session — the collapse that killed run
`r-20260828-01` on the web side, with no per-agent isolation available to fix
it. The runner refuses a second.

## Driving the app

Everything goes through `scripts/explore/ios.sh`:

```bash
scripts/explore/ios.sh up          # boot, build, install, launch
scripts/explore/ios.sh state       # {online, running, forced_offline}
scripts/explore/ios.sh outbox      # the queued mutations, as JSON
scripts/explore/ios.sh screenshot  # a PNG you can look at
```

Taps and typing are not in that script — use whatever simulator control your
harness gives you, and read the screen from `screenshot`. The prime directive
still holds: **you navigate by tapping**, never by deep link. The only URL you
may send the app is the offline switch below, and that is a device control,
not a destination.

## Naming the book

No screen in the native app shows a book's uuid, so you cannot fill the
journal's `target` field the way a web agent does. Leave `target` null and put
the book's **exact title**, as the app displays it, in `params.title` on every
entry about that book. The audit resolves the title against the library and
fills the uuid in for you — provided exactly one book carries it. When two do
(two editions of one title), say so in the entry's `note` and expect the audit
to decline that entry rather than guess. Ownership is unaffected: it comes from
`book.add`, and this surface never runs `adding_book`.

## The offline switch

`scripts/explore/ios.sh offline on|off`. It is DEBUG-only and exists in no
shipped build. It fails the app's whole `/api/*` client with the same error a
real unreachable server produces, and takes the health probe down with it, so
nothing quietly reconnects underneath you.

Two things to know before you use it:

- **It relaunches the app.** Flip it between screens, not in the middle of
  one — go offline first, *then* open the book. (`offline-url` flips without a
  relaunch, but iOS sometimes puts up an "Open in Omnibus?" confirmation that
  something has to tap. Use it only when you can see and tap the screen, and
  fall back to `offline` the moment it does not take.)
- **Never trust it silently.** `ios.sh offline` polls the app's own readback
  and fails loudly if the switch did not take, and `ios.sh state` answers the
  same question at any time. A scenario that believes it went offline and did
  not is the one failure that produces a clean-looking pass over a test that
  never ran — so read the answer, and journal what it said.

What the switch does **not** cover: a download already in flight. The
background transfer session is not the `/api/*` client. An audiobook download
started while offline still fails (its manifest read goes through the client),
but a single-file ebook fetch may complete. That is a limit of the switch, not
a defect in the app — do not journal it as one.

## Flow: offline outbox

You are handed this once, and you wrap one of your ordinary flows in it. It is
the reason the iOS lane exists: nothing else in the system exercises the
mutation outbox, and its contract — every queued write lands, exactly once — is
not observable from the web surface.

### Steps

1. **Online, on the shelf you will use.** Open a book you are about to write
   to and note what it currently shows: position, read status, rating,
   highlights, bookmarks, journal entries. Journal that as your baseline; the
   audit compares against it.

   **Then download it, while you still can.** The reader serves the downloaded
   file when there is one and otherwise fetches the book over `/api/*` — which
   is exactly the client the offline switch fails. A book you did not download
   cannot be opened offline at all, so an agent who skips this reaches step 3
   with nothing on the device and no way to perform it. Use the book detail
   screen's **Download** control and wait for it to finish before you flip the
   switch; a download started *after* going offline is the case the switch
   deliberately does not cover.
2. **Go offline** (`ios.sh offline on`) and confirm the app says so — an
   "Offline" pill appears in the **Library tab's** masthead.

   **That pill lives on the Library tab and nowhere else.** The You tab, the
   book detail screen and the reader show no connectivity indicator of any
   kind, so an agent checking from one of those sees nothing and concludes the
   switch failed. Go to Library to read the pill — and either way, believe
   `ios.sh state` over the screen.
3. **Write, as a reader would.** Read a few pages and let the position move.
   Save a highlight. Save a bookmark — the EPUB reader has an **Add bookmark**
   control and so does the audiobook player, and they write the same model
   through the same endpoint, so use whichever reader you already have open.
   Write a journal entry.
   Add a book to a shelf. Not all of them every time — pick two or three and
   do them properly.
4. **Look at what you wrote.** Every one of them must be visible in the app
   immediately: the highlight in the list, the journal entry on the book, the
   position where you left it. Anything that is not is a **fail**, not a
   pending state — the outbox applies optimistically.
5. **Confirm the queue holds them**: `ios.sh outbox`. The pill should read
   "Offline · N queued" and N should match. Journal the queue.
6. **Kill and relaunch** (`ios.sh relaunch`). The queue must survive, still
   offline, still N. A write that vanishes here was never durable.
7. **Come back** (`ios.sh offline off`). The pill turns to "Syncing N", then
   goes.

   **Not seeing "Syncing" is not a failure.** The pill shows that state only
   while writes are still in flight, and a handful of queued mutations drains
   in a couple of seconds — under ~25s in every run so far, and often far
   less. Missing the window means the drain was fast, not that it did not
   happen. Step 8 is what proves the drain; the pill is a courtesy. Only an
   `outbox` that stays non-empty is a finding.
8. **Verify on the server.** Reopen the book and check every write you made is
   there, once. Then check `ios.sh outbox` is empty.

### The probes — writes that must refuse

Two of them, deliberately chosen because both are safe to attempt and neither
leaves anything behind when it correctly fails. While offline:

- **Create a shelf.** The device cannot name a shelf it has not created, so
  this is not queueable. It must fail with a visible message.
- **Change your display name** (You → edit profile). Account configuration,
  never queued. The Save control should be *disabled* while offline; that
  counts as failing visibly.

Journal each as `outcome: refused` with what you saw. A probe that appears to
succeed offline is a **high**-severity finding: it means a write was accepted
that nothing will ever deliver, or one that will be replayed hours later over
a value someone has since changed.

Do not probe reindex, send-to-Kindle, or anything under Settings. Those are on
`start.md`'s rails for reasons that have nothing to do with being offline.

### Journal

Use these actions, and no near-variants — the audit matches on them:

| Action | When | `params` must carry |
|---|---|---|
| `offline.on` / `offline.off` | each flip | `readback` (what `ios.sh state` said) |
| `outbox.queued` | after step 5 | `count`, and `kinds` from `ios.sh outbox` |
| `outbox.drained` | after step 7 | `count` (should be 0) |
| `<verb>.create` etc. | each write | exactly what `start.md` says for that flow |
| `probe.refused` | each refusal | `what`, `observed` |

**Give every write a value nothing else could have produced** — a highlight
note, a journal body, a shelf name — and write it the way `start.md`'s *What
you type into the app* says: a memorable phrase, never the run id, your actor
id or your `seq`. That rule holds on this surface exactly as on the web. An
earlier version of this file asked for the run id in shelf names, and the two
shelves it produced are still in the shared library for every reader to see
(#2364); do not add to them. A phrase you would recognise on sight is unique
enough: the audit tells "landed once" from "landed twice" by matching that
content, and a duplicate is two server rows carrying it.

### Pass

- Every offline write was visible locally the moment you made it.
- The queue survived a relaunch.
- After reconnecting, every write is on the server exactly once, and the queue
  is empty.
- Both refusal probes failed visibly.

### Fail

- A write vanished — at the relaunch, or after the drain.
- A write landed **twice**. This is the finding the lane exists to catch.
- The queue never emptied, or emptied while writes did not land.
- A refusal probe appeared to succeed.
- The app reported itself online while the switch said otherwise.

### Sharp edges

- Positions coalesce **on purpose**: reading for five minutes offline queues
  one position, not fifty, and the server should end up at the last one. One
  queued position for one book is correct, not a lost write.
- The reader and the player are *meant* to write read status on their own —
  opening a book marks it `reading`, finishing marks it `finished`. Where it
  happens, do not journal it as a write you made, and do not call it unexpected
  when the audit sees it. **On this surface it currently does not happen**
  (#2289): a book read in the native reader comes back with no read status at
  all, while the Library's continue card still shows it as Reading. Treat an
  unchanged status on iOS as that known bug rather than a fresh finding, and do
  not lean on the transition to set up a status you need — set it by hand from
  the detail screen's status control.
- A drained position answer can come back *different* from what you sent. The
  server resolves position conflicts, so another device's newer position
  winning is correct behaviour.
