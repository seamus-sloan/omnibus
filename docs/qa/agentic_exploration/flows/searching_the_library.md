# Searching the library

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `search.query`, `search.open` |

Find things the way a reader who half-remembers them does. Search writes
nothing, so it is cheap — and it is where a metadata edit made an hour ago
either shows up or silently does not.

## Preconditions

A library with enough books that a query can miss. If there are fewer than
three or four, journal that and end the flow `uncertain`.

## Steps

1. Open search from the nav. On the web it is a command palette; typing into
   it shows grouped results as you go, and pressing Enter opens the full
   results page with sections for **Books**, **Authors**, **Series**, **Tags**
   and **Genres**.
2. Search for a **title word** from a book you have seen in the library. Not
   the whole title — one distinctive word from the middle of it.
3. Search for an **author's surname** and confirm the author appears in the
   Authors section and their books in the Books section.
4. Search for a **series name** and confirm the series appears.
5. Search for a **tag** and a **genre** you saw on a book's detail page —
   one of each, and prefer a tag or genre that a reader **added through the
   app** over one that came from a file, because the two are indexed
   differently and the added one is where the gap has been.
6. Search for something that should match **nothing** — a word from no book
   you know of — and confirm the page says so rather than showing an empty
   frame or stale results.
7. Misspell one of the earlier queries by a letter and note whether anything
   comes back. Fuzzy matching is not promised; journal what you saw as an
   observation, not a finding. Then search an **accented** name with and
   without its accents (an author like Pérez Galdós) and note which sections
   match each spelling.
8. **If you or another agent edited a book's metadata earlier in the run**,
   search for the *new* value and for the *old* one. The edited value is what
   the library shows, so it is what search should find. The old value still
   matching is worth journalling, but as `uncertain`.
9. Open a result of each kind and confirm you land on the thing you clicked —
   the book, the author, the series. On the results page also try the
   controls beside the sections — the sort and view buttons and the "On this
   page" rail — and drive the palette from the keyboard: arrows, Enter,
   Escape.
10. If you or another agent added a book or renamed one this run, search for
    it: a new book should be findable within a couple of minutes, and a
    renamed one under its **current** title only.

**Do not search for book text.** The palette's full-text section is marked
"Coming soon", and journal entries are not indexed either. A passage from
inside a book returning nothing is not a finding.

## Journal

`search.query` with the exact query, which section you expected it in, and
the results shown — titles and section, in order. `search.open` with what you
clicked and where you landed. A query that returned nothing gets the same
entry with an empty result list.

## Pass

- A title word, an author, a series, and a tag each find what they name.
- An empty result is reported as empty.
- Opening a result lands on it.
- The results agree with the library: a book a search finds is in the grid,
  and an author it names has an index entry.

## Fail

- A book visibly in the library that its own title word cannot find.
- A result that opens the wrong book, author or series.
- Results from a query you did not type — stale results surviving a new query.
- A search that errors, hangs, or leaves the palette stuck open.
- A book another agent added a while ago that never becomes searchable. Give
  indexing a moment first.

## Sharp edges

- Search is over the **effective** metadata — the edited value where one
  exists, the file's value otherwise. Which one it finds after an edit is
  the interesting question in step 8.
- Results are grouped, and the group order can change with the query — a tag
  match may lead when the query is a tag. That is intended.
- The command palette searches content, not actions. Typing "add a shelf"
  into it finds nothing, and that is correct.
- On iOS search is its own tab; the sections are the same.

## Corrections from run r-20260908-02

- **Step 8 presumes an edit that replaced a value.** `editing_metadata` permits
  additions only, so there is usually a new value to search for and no old one.
  Read the step as "if your edit replaced a value, search for the old one
  too", and say plainly when it did not. Its precondition — that some agent has
  edited metadata at all — is only checkable in the run journal, not in the
  app: look for a `metadata.save` by another actor before deciding the step is
  not runnable.
- **Step 9 should open a tag and a genre as well as a book, an author and a
  series.** Step 1 names five sections and step 9 names three, and the missing
  two are where #2504 lives.
- **Step 5 cannot be answered from inside the app.** Nothing distinguishes a
  tag a reader added from one that came out of the file's subject list; the
  chips render identically. Add a tag yourself first, or ask the runner which
  is which.
- **The Sort and View controls on the results page are known to be inert**
  (#2514's sibling finding). Note whether they respond and move on; three
  agents have now proved it in turn.
- **The palette's empty state is a bare "0 results"**; only the full results
  page says "No results for …". Step 6's criterion is about the page.
- **On iOS**, search is its own tab, results render live under the field with
  no Enter and no results page, an overflowing section carries an `All N ›`
  link rather than a count you can page through, and there is **no Genres
  section at all** (#2515). Sort, view, the "On this page" rail and the
  keyboard steps have no iOS surface — mark them `uncertain`, not failed.
