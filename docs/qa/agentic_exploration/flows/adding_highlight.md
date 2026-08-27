# Adding a highlight

| | |
|---|---|
| **Weight** | 50% of a reading flow |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `highlight.create`, `highlight.note`, `highlight.recolour`, `highlight.delete` |

Runs inside [reading_a_book](reading_a_book.md), not on its own. Highlights are
per-user content: yours are yours, and another agent highlighting the same book
must never affect what you see.

## Steps

1. While reading, select a passage — a sentence or two, not a single word and
   not a whole chapter.
2. Save it as a highlight. Pick a colour if offered one.
3. **Half the time, attach a note.** Write something a person would write, and
   include a distinctive word so you can find it again later. Journal the exact
   text.
4. Carry on reading for a page or two.
5. Open the list of saved passages and confirm yours is there, with its colour
   and note intact.
6. Change the colour and confirm it sticks.
7. **Delete a highlight only if you have made more than one in this run**, and
   never the last one standing. Every highlight you leave behind is evidence the
   post-run audit reconciles against; deleting your only one erases the thing
   being checked. When you skip the delete for this reason, journal that you
   skipped it and why — a skipped step recorded is data, a silent one is a gap.

## Journal

`highlight.create` carrying the book uuid, the **verbatim selected text**, the
location in human terms, and the colour. If you added a note, `highlight.note`
with the exact note text. The verbatim text is what the audit matches on, so
copy it rather than paraphrasing.

Deletions get `highlight.delete` with the same identifying text — an
intentional delete and a silent data loss look identical to the audit
otherwise.

## Pass

- The highlight appears immediately over the passage you selected — that
  passage, not a neighbouring one.
- It survives leaving the reader and coming back.
- The note is attached to the right highlight, with the text you typed.
- It appears in the book's saved-passages list on the detail page.
- Colour changes and deletions persist.

## Fail

- The highlight lands on different text than you selected, or spans the wrong
  range.
- It disappears after a reload.
- A note attaches to the wrong highlight, or comes back empty or truncated.
- You see a highlight you did not create. **Report this at high severity** —
  highlights are per-user and cross-user leakage is the most serious thing this
  flow can find.
- The saved-passages list disagrees with the reader about what exists.

## Sharp edges

- Selecting across a paragraph or page boundary is legitimately awkward. Try it
  deliberately once in a while, but journal it as `uncertain` rather than
  `fail` unless the result is plainly wrong.
- Another agent's highlights on the same book are invisible to you by design.
  Seeing none from anyone else is correct.
