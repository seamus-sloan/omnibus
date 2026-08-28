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
2. Write something a person would write about a book — a paragraph of actual
   opinion, not a test string, and **more than one paragraph at least half the
   time**. Work in a **phrase you will recognise again** and journal it, so the
   audit can match the entry later; a memorable phrase, never a generated token
   (see *What you type into the app* in [start.md](../start.md)). Do **not**
   expect to find it via search: the command palette's full-text section is
   marked "Coming soon" and journal text is not indexed, so a search returning
   nothing is not a finding.
3. Use formatting, and **rotate which kind across runs** — the editor offers far
   more than bold. Pick two or three from the table below rather than reaching
   for bold every time.
4. Occasionally embed an image. The corpus carries one beside most books —
   `cover.jpg`, in the same folder as the book file — and that is the one to
   use; there is no separate image library. Inserting one drops an
   `Add a caption` placeholder in: **replace it with a real caption**, because
   that text becomes the image's visible caption. Try it both ways across runs
   — an image alone on its own line, and an image in the middle of a sentence —
   they are meant to render differently.
5. Save it, and confirm it renders — the formatting applied, the text intact.
6. Come back later in the run, find it again, and confirm it is unchanged.
7. Occasionally edit it and confirm the edit sticks; occasionally delete it and
   confirm it goes.

## What to format with

Rotate through these. The first column is what you type; several have a toolbar
button too, and finding the button is itself worth doing sometimes.

| | What it should do |
|---|---|
| `**bold**`, `*italic*` | The obvious two. There is **no underline** — if you find a control offering one, that is the finding. |
| `~~struck through~~` | Renders struck through. |
| `` `inline code` `` | Renders in a monospace font. |
| `# Title`, `## Subtitle` | A heading, larger than body text. Only these two are decorated as you type; a deeper `### ` is still a heading once saved, and that difference is expected. |
| `> a quoted line` | An indented quote. Saved passages inserted from the highlights picker arrive as one of these. |
| `- item`, `1. item` | Bullet and numbered lists. |
| `- [ ] task` | A checkbox. It is meant to be **read-only** — clicking it in a saved entry must do nothing. |
| `[text](https://example.com)` | A link. |
| `\|\|spoiler\|\|` | Hidden until revealed. The hint sits under the composer. |
| a single newline | A visible line break, **not** a collapsed space. A blank line starts a new paragraph. Both must survive saving exactly as you typed them. |

**On iOS, only the first four rows apply.** The iOS card renders the entry
inline-only, so bold, italic, code, strikethrough and links come out formatted
while headings, quotes, lists, checkboxes and spoilers stay on screen as the
literal characters you typed. That is current behaviour on that surface, not a
defect — do not report it. Everything below is a **web** criterion.

Two worth deliberately trying on web, because each has been wrong before:

- **Spoilers are a button, not a blur.** Tab to it and press Enter — it should
  reveal that way, not only on a mouse click. An unpaired trailing `||` is meant
  to stay on screen as two literal pipes.
- **Line breaks are load-bearing.** Type a three-line stanza with single
  newlines and confirm it saves as three lines. Collapsing them into one
  paragraph is a real defect, not a markdown nicety.

## Journal

`journal.create` with the uuid and the **verbatim text you typed**, including
the markup. `journal.update` with before and after. `journal.delete` with the phrase you
chose, so the audit can tell a deliberate delete from a loss.

## Pass

- The entry saves and appears without a reload.
- In the **saved** entry, formatting renders as formatting rather than as
  visible markup characters (subject to the iOS caveat above).
- Line breaks and paragraph breaks survive exactly as you typed them.
- On web, a spoiler is hidden until revealed, and reveals from the keyboard as
  well as the mouse.
- On web, a checkbox from a `- [ ]` line is inert.
- An embedded image displays, and a caption you wrote appears with it.
- The text is exactly what you typed — no truncation, no escaped characters
  where there should be none.
- It is still there, unchanged, when you come back.

## Fail

- The **saved** entry shows raw markup, or renders the wrong formatting. (The
  composer is a different matter — see *Sharp edges*.)
- Multiple lines collapse into one paragraph, or a blank line is swallowed.
- **On web**, a spoiler renders as literal `||` pipes, or reveals its text
  before it is activated.
- **On web**, a checkbox is clickable, or a heading renders at body size.
  (On iOS both are expected to be literal text — see above.)
- Text is truncated, reordered, or has characters mangled.
- An image uploads but does not display, displays on another entry, or keeps the
  `Add a caption` placeholder as its caption after you replaced it.
- The entry attaches to the wrong book.
- An entry is attributed to the wrong reader, or your own entry renders without
  the "you" marker. (Merely *seeing* another reader's entry is expected — the
  journal is a shared surface.)
- An edit silently discards the previous content.

## Sharp edges

- **The composer shows markdown characters on purpose.** It is a live editor,
  not a preview: the `**` and `> ` stay in the text and fade out except on the
  line your caret is on. Seeing them while writing is correct, and only the
  **saved** entry is judged on rendering. Use the **Preview** tab if you want to
  see it rendered before publishing.
- On iOS an entry may briefly show its markdown source before the server
  returns the rendered version. That is deliberate — the client will not guess
  at rendering it locally. Only a source view that **persists** is a finding.
- Apostrophes and quotation marks are a rich source of real bugs. Include some
  on purpose and read the result carefully.
- Long entries and entries with several images are worth trying occasionally;
  say so in the journal when you do.
