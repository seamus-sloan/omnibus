# Adding a bookmark

| | |
|---|---|
| **Weight** | 50% of a listening flow |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `bookmark.create`, `bookmark.delete` |

Runs inside [listening_to_audiobook](listening_to_audiobook.md). A bookmark is
a named moment; the thing worth testing is whether the timestamp it stores is
the one you were actually at.

## Steps

1. While playing, note the exact position shown — the elapsed time and the
   chapter or file.
2. Save a bookmark there.
3. Keep listening for a minute or two so the position moves well past it.
4. Open the list of bookmarks and confirm yours is present at the time you
   saved it.
5. Occasionally, jump to it and confirm playback resumes at that moment rather
   than a few seconds either side.
6. Occasionally, delete it and confirm it goes.

## Journal

`bookmark.create` with the book uuid, the elapsed time **as displayed**, the
chapter or file, and any label. Record the displayed time before you press the
control, not after — the two differ by however long the interaction took, and
that difference is exactly what a rounding bug hides in.

## Pass

- A confirmation appears and the bookmark is in the list.
- Its stored time matches where you were, within a couple of seconds.
- Jumping to it lands you back at that moment.
- It survives leaving the player and returning.

## Fail

- The bookmark saved at a different time than displayed — a whole chapter off,
  or at zero.
- It vanishes after a reload.
- Jumping to it starts a different file, or a different book.
- A bookmark you did not create appears in your list.

## Sharp edges

- A couple of seconds of drift between pressing and saving is not a defect;
  a minute is.
- Bookmarks in an audiobook and highlights in an ebook are separate things
  stored separately, even for a book that exists in both formats. Neither
  should appear in the other's list.
