# Listening to an audiobook

| | |
|---|---|
| **Weight** | 15% |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.open`, `player.play`, `player.seek`, `player.rate`, `player.close` |

Listen to roughly a tenth of an audiobook **by position** — a tenth of a ten-hour
book is an hour of playback you should skip through, not an hour of real time
you sit and wait. Playback position, like reading
position, is written constantly and missed immediately when lost.

## Preconditions

A book with an audiobook format. Multi-file audiobooks are more interesting
than single-file ones, but **the library gives you no way to tell before you
open the player** — the FORMATS column shows only `M4B`, and the file count
appears nowhere until you are inside. Open one, see what you got, and say which
in the journal. A single-file M4B can still expose many chapter markers, so
chapter seams are testable even when file seams are not.

## Steps

1. Reach the book and start listening.
2. **Skip ahead to roughly 10% of the book** rather than listening through to
   it — a tenth of a ten-hour audiobook is an hour of wall clock, and nothing
   here tests your patience. Play a stretch at each place you land so you can
   hear that audio actually runs, and use the controls the way a listener does:
   skip back thirty seconds after losing the thread, skip forward past
   something dull.
3. Change the playback speed at least once, and let it play on at the new rate.
4. If the book has several files or chapters, cross at least one boundary and
   watch what happens at the seam.
5. Occasionally set a sleep timer and watch it count down; you need not wait
   for it to fire.
6. **Go back to the library, then close the mini-player.** Follow the book title
   out of the player, then use the persistent mini-player's "Stop and close
   player". There is no single exit control, and leaving via the title does
   **not** stop playback — the mini-player keeps going, which is intended.
   Then check the book's detail page reflects where you got to.

## Journal

`player.play` with uuid, file or chapter, and starting position.
`player.seek` for each jump, carrying both the from and the to. `player.rate`
on a speed change with the old and new values. `player.close` with the final
position, file, and rate.

## Pass

- Audio starts within a few seconds and plays continuously.
- Elapsed and remaining times advance sensibly and agree with each other.
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
- A book that exists as both an ebook and an audiobook keeps **separate**
  positions for each. Reading position not moving because you listened is
  correct.
