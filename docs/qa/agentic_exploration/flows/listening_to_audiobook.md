# Listening to an audiobook

| | |
|---|---|
| **Weight** | 15% |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.open`, `player.play`, `player.seek`, `player.rate`, `player.close` |

Listen to roughly a tenth of an audiobook. Playback position, like reading
position, is written constantly and missed immediately when lost.

## Preconditions

A book with an audiobook format. Multi-file audiobooks are more interesting
than single-file ones — prefer them when you can tell.

## Steps

1. Reach the book and start listening.
2. Let it play. Do not sit in silence watching a counter; use the controls the
   way a listener does — skip back thirty seconds after losing the thread, skip
   forward past something dull.
3. Change the playback speed at least once, and let it play on at the new rate.
4. If the book has several files or chapters, cross at least one boundary and
   watch what happens at the seam.
5. Occasionally set a sleep timer and watch it count down; you need not wait
   for it to fire.
6. Leave the player through the app's own way out, then check the book's detail
   page reflects where you got to.

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
