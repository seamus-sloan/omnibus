# Listening to an audiobook

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.open`, `player.play`, `player.seek`, `player.rate`, `player.sleep`, `player.close` |

Listen to roughly a tenth of an audiobook **by position** — a tenth of a ten-hour
book is an hour of playback you should skip through, not an hour of real time
you sit and wait. Playback position, like reading
position, is written constantly and missed immediately when lost.

## Preconditions

A book with an audiobook format. Multi-file audiobooks are more interesting
than single-file ones. The FORMATS column shows only `M4B`, but **the detail
page's THE FILES section lists every file** — one `Audiobook · 298.2 MB` row
for a single-file book — so read the count there before you open the player.
Do not try to read it from inside: the Chapters panel lists chapter names, and
on a book whose chapters are named "Odyssey - 1 … 22" that reads exactly like
22 files when it is one. An agent in run r-20260908-02 reported crossing file
boundaries it could not have crossed for precisely that reason. A single-file M4B can still expose many chapter markers, so
chapter seams are testable even when file seams are not.

## Steps

1. Reach the book and start listening.
2. **Advance by roughly a tenth of the book** (or to at least 10% if you are
   starting from the beginning) rather than listening through to it — a tenth of a ten-hour audiobook is an hour of wall clock, and nothing
   here tests your patience. Play a stretch at each place you land so you can
   hear that audio actually runs, and use the controls the way a listener does:
   skip back thirty seconds after losing the thread, skip forward past
   something dull.
3. Change the playback speed at least once, and let it play on at the new rate.
4. If the book has several files or chapters, cross at least one boundary and
   watch what happens at the seam.
5. Occasionally set a sleep timer and watch it count down; you need not wait
   for it to fire — though "End of chapter" armed just before one of a book's
   short early chapters fires inside the budget, which is worth seeing once.
   Journal it as `player.sleep`. Closing the player cancels it.
6. **Leave the player, then close the mini-player.** On the web, follow the
   book title out of the player — it goes to the book's **detail page**, not
   the library — then use the persistent mini-player's "Stop and close
   player". There is no single exit control, and leaving via the title does
   **not** stop playback — the mini-player keeps going, which is intended. On
   iOS the top-left chevron minimises the player; the mini-player shows only
   above the Library tab's bar and closes with its **X**. Then check the
   book's detail page reflects where you got to.

## Journal

`player.play` with uuid, file or chapter, and starting position.
`player.seek` for each jump, carrying both the from and the to. `player.rate`
on a speed change with the old and new values. `player.close` with the final
position, file, and rate.

## Pass

- Audio starts within a few seconds and plays continuously.
- Elapsed and remaining times advance sensibly and agree with each other —
  at 1.0×. At any other rate, remaining is rate-adjusted wall time while
  elapsed is book time, so the two do not sum to the total; that is the
  intended clock, not a fault.
- Skip controls move by the amount they advertise.
- A speed change takes effect and is still in force after leaving and
  returning.
- Crossing a file boundary continues into the next one without a gap, a
  restart, or a jump to the wrong file.
- The detail page afterwards shows roughly where you stopped.

## Fail

- Playback stalls, or the position counter advances while no audio plays.
- Position resets to zero, or jumps to a different file, on its own.
- A file boundary restarts the book, skips a file, or plays the same one twice.
- The chosen speed reverts on its own.
- Returning to the book starts it from the beginning.

## Sharp edges

- **First play sets read status to reading**, and finishing every file marks it
  finished. Both are automatic.
- The first few seconds may buffer while the server prepares the audio. Give it
  a moment before calling it a stall.
- The chapters panel lists chapter **durations**, not start offsets, so the
  seam you want to cross has to be added up. While that panel, or the speed
  or sleep panel, is open its scrim covers the transport; close it (its own
  Close, or a click on the scrim) before the skip buttons will answer.
- The title/author link that leaves the player sits outside `<main>`.
- A book that exists as both an ebook and an audiobook keeps **separate**
  positions for each. Reading position not moving because you listened is
  correct.

## Two corrections from run r-20260908-02

- **The speed and sleep panels have no Close button.** Only Chapters and
  Bookmarks do. The scrim is the only way out of those two, and their own
  trigger sits underneath it, so it cannot toggle them shut either. Three
  agents lost time to the sharp edge that says otherwise.
- **The shortest *timed* sleep option is 15 minutes**, so no timed sleep can
  be watched to fire inside a session's budget. "End of chapter" is the only
  candidate, and on the web it is currently broken (#2494) while on iOS it
  works — so a web agent's step 5 is expected to fail and an iOS agent's is
  not. The sleep panel also offers a **Fade out volume** option this document
  never mentioned, and the fade runs whether or not you choose it.
