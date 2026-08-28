# Writing a journal entry

| | |
|---|---|
| **Weight** | 25% of a details flow |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `journal.create`, `journal.update`, `journal.delete` |

A journal entry is a piece of writing about a book, rendered from markdown by
the server, optionally carrying images — three things that can each break
independently.

**It is not private.** The app presents the journal as a shared, attributed
per-book surface ("THE JOURNAL · 1 ENTRY FROM 1 READER", with a "· you" marker
on your own byline). Seeing another reader's entry is therefore expected, not a
leak. What *would* be a finding is an entry attributed to the wrong person, or
your own entry appearing without the "you" marker.

## Steps

1. From a book's detail page, find where entries about the book are written.
2. Write something a person would write about a book — a paragraph, not a test
   string. Include a **distinctive phrase** and journal it, so the audit can
   match the entry later. Do **not** expect to find it via search: the command
   palette's full-text section is marked "Coming soon" and journal text is not
   indexed, so a search returning nothing is not a finding.
3. Use at least one piece of formatting: bold, italic, a quote, or a list. Vary
   which one across runs.
4. Occasionally embed an image from the corpus.
5. Save it, and confirm it renders — the formatting applied, the text intact.
6. Come back later in the run, find it again, and confirm it is unchanged.
7. Occasionally edit it and confirm the edit sticks; occasionally delete it and
   confirm it goes.

## Journal

`journal.create` with the uuid and the **verbatim text you typed**, including
the markup. `journal.update` with before and after. `journal.delete` with the
distinctive phrase, so the audit can tell a deliberate delete from a loss.

## Pass

- The entry saves and appears without a reload.
- Formatting renders as formatting, not as visible markup characters.
- An embedded image displays.
- The text is exactly what you typed — no truncation, no escaped characters
  where there should be none.
- It is still there, unchanged, when you come back.

## Fail

- The entry saves but shows raw markup, or renders the wrong formatting.
- Text is truncated, reordered, or has characters mangled.
- An image uploads but does not display, or displays on another entry.
- The entry attaches to the wrong book.
- An entry is attributed to the wrong reader, or your own entry renders without
  the "you" marker. (Merely *seeing* another reader's entry is expected — the
  journal is a shared surface.)
- An edit silently discards the previous content.

## Sharp edges

- On iOS an entry may briefly show its markdown source before the server
  returns the rendered version. That is deliberate — the client will not guess
  at rendering it locally. Only a source view that **persists** is a finding.
- Apostrophes and quotation marks are a rich source of real bugs. Include some
  on purpose and read the result carefully.
- Long entries and entries with several images are worth trying occasionally;
  say so in the journal when you do.
