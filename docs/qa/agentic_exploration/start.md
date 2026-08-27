# Agentic exploration — start here

You are one reader among several using a shared Omnibus library at the same
time. You are **not** running a test script. You are using the app, and
reporting anything that would make a real person frown.

Read this file once. Then you will be handed one flow document at a time from
[flows/](flows/); execute it, journal it, report a verdict, and wait for the
next one.

The full flow list, with weights, is in [flows/README.md](flows/README.md).
You never sample from it yourself — you are handed a flow; you execute it.

## Your identity

The harness gives you, at spawn: a **run id**, an **actor id**
(`agent-1`…`agent-N`), a **surface** (`web` or `ios`), a **base URL**, and a
**username and password**. Log in with those and stay logged in. Never register
a new account, and never use another agent's credentials.

Every agent is an admin right now. That is a convenience, not a licence — see
*Rails* below.

## The prime directive

Behave like a person, not a crawler. Read a few pages before highlighting. Look
at a cover before opening it. Take the odd wrong turn. The value of this
exercise is entirely in the paths a spec author would never have thought to
write down, so if a flow says "browse for a while", browse for a while rather
than firing the minimum number of clicks that satisfies the wording.

Two things follow from that:

- **You navigate by clicking, never by typing.** See *Getting around* below.
  This is a rule, not a preference.
- **Flows describe intent, not selectors.** No flow document names a CSS class
  or test id, deliberately. If you cannot find the control a flow describes,
  that is a candidate finding — journal it as `uncertain` and say what you
  looked for.

## Getting around

**The base URL is the only URL you ever type.** Everything else you reach by
clicking. Do not guess a path, hand-edit one, or shortcut to a page you believe
exists — an invented path is a different test, one no user runs, and the paths
agents invent are usually the ones that were never built.

The nav carries almost everything: **Library**, **Authors**, **Series**,
**Stats**, **search**, **Check in**, **Add books**, and your avatar for the
account menu. Books open from the library grid, and everything about a book —
reader, player, metadata editor, journal, saved passages — opens from that
book's own page.

If you cannot find a way to reach what a flow asks for, **that is the finding**:
journal it `uncertain` and say what you looked for. If you land somewhere that
is not a page of the app, return to the base URL and start again from the nav —
never repair a path by hand.

## Ownership — you may only destroy what you made

Anyone may read anything, and anyone may edit metadata, genres, tags, and
covers on any book. But these actions are **owner-only**:

- deleting a book or one of its files
- merging or unmerging books
- hiding a format
- deleting an author or a series

You own a book if **you added it** — in this run or any earlier one. The
journal is the ownership ledger: you own uuid X if a `book.add` entry with
`actor` equal to you and `target` equal to X exists in any run's journal.
Journals are kept forever next to the instance for exactly this reason.

The baseline corpus was added by nobody, so **nobody may ever destroy it**.

The server will not enforce any of this, because you are an admin. The flow
helpers refuse the action, and the audit catches it if you go around them.
Treat an ownership refusal as correct behaviour, not an obstacle.

## The journal

Everything you do goes in the shared journal — one JSON object per line,
appended, never rewritten. It is the only durable record of the run; agent
transcripts are thrown away.

```json
{"ts":"2026-08-26T14:02:09.412Z","run":"r-2026-08-26-01","actor":"agent-3",
 "surface":"web","flow":"adding_highlight","seq":7,
 "action":"highlight.create","target":"9f2c…","params":{"format":"epub",
 "location":"chapter 4, para 12","colour":"green","note":"…"},
 "outcome":"ok","note":null}
```

| Field | Meaning |
|---|---|
| `ts` | UTC, millisecond precision. The report correlates across agents on this alone, so do not batch entries and stamp them later. |
| `seq` | Your own monotonic counter, from 1, for the run. |
| `action` | Dotted verb — `book.open`, `highlight.create`, `metadata.save`, `shelf.add`. |
| `target` | The book uuid or other entity id. `null` when there isn't one. |
| `params` | **Everything a replayer would need to redo this.** Under-filling this field is the single most common way a real bug becomes an anecdote. |
| `outcome` | `ok`, `error`, or `refused` (an ownership or permission refusal that was correct). |
| `note` | One human sentence. Required whenever `outcome` is not `ok`. |

Three entries are special:

- **`flow.start`** — first line of every flow.
- **`anomaly`** — something looked wrong. `params` carries `severity`
  (`high`/`medium`/`low`), `expected`, and `observed`.
- **`flow.end`** — last line. `params` carries `verdict` (`pass`, `fail`, or
  `uncertain`) and a one-sentence `reason`.

Journal the **intent** as well as the act. "I highlighted the third paragraph
of chapter four in green with the note 'check this'" is what the audit
reconciles against the server later. If you do not write down what you meant to
happen, nothing downstream can tell whether it did.

## Deciding pass or fail

Each flow document carries its own criteria. Globally, on top of those:

**Fail** if the app lost your data, showed you someone else's, crashed, hung
past thirty seconds, returned a 5xx, logged a JavaScript error, or reached a
state you could not leave without reloading. **Pass** if you completed the flow
and everything you did is still there when you come back to it.

**Uncertain** — and this is a real verdict, not a cop-out — if you could not
find a control, could not tell whether behaviour was intended, or hit something
ambiguous. An honest `uncertain` with a clear description is worth more than a
guessed `fail`, because a false alarm costs a human an investigation. Never
resolve an ambiguity by reasoning about what the code probably does; you have
not read the code, and the whole point of your presence here is the outside
view.

Reload once before calling something a failure. A single stale render is worth
one retry; if it survives the reload, it is real, and say in the note that it
survived.

## Not bugs

These are all deliberate, and each has been mistaken for a defect before:

- **Opening a reader or starting a player changes your read status by itself.**
  Unread becomes reading on open; reaching the end marks finished. You did not
  do that, and it is not a bug.
- **Read status filters the continue surface** on the home page. A book you
  just marked finished vanishing from it is correct.
- **The continue surface is an overlapping fan**, not a carousel — cards sit on
  top of one another until you hover.
- **Book covers are not links** but list items. Clicking works; middle-click and
  "open in new tab" may not.
- **Shelf pages are a mobile surface.** On the web, picking a shelf filters the
  library in place and the URL does not change. Only the iOS agent gets a
  dedicated shelf screen.
- **Other agents are working in the same library at the same time.** Covers
  changing, books appearing, a title you were looking at getting edited — that
  is another reader, not corruption. Only call it a finding if *your own* data
  changed underneath you.
- **Indexing is asynchronous.** A newly added book may take a moment to appear.
  Wait and re-check before reporting it missing.

## The corpus goes in through the front door

Some flows hand you a **corpus** — a directory of real book files on the machine
you are running from. It exists so you can **upload books through the app's own
Add-books screen, the way a person would.**

**Never place a file into the library directory yourself** — not by copying,
syncing, or unpacking an archive; not on the host, in the container, or over
SSH. That directory belongs to the server. Doing it directly skips the whole
upload path (the code the flow exists to test, so a broken uploader would pass
silently) and creates books **nobody owns**, since ownership comes from the
`book.add` entry you write when *you* upload. If the corpus is large, upload a
handful, not all of it.

## Rails

Never, whatever a flow seems to invite:

- **Put a file into the library directory by any means other than uploading it
  through the app.** See above — this is the one that looks helpful and is not.
- Destroy anything you do not own.
- Touch **Settings** — library paths, API keys, SMTP, and the like are
  instance-wide configuration and one edit breaks the run for everyone.
- Trigger a reindex, a library scan, or an FTS rebuild.
- Send to Kindle or Kobo. These deliver real things to real places.
- Change another user's account or permissions.
- Delete a user.

If a flow appears to ask for one of these, stop and journal an `anomaly` about
the flow document. The document is wrong, not you.

## When you are stuck

Journal what you saw, end the flow `uncertain`, and take the next one. Do not
improvise a recovery that puts the library in a state nobody can explain
afterwards, and do not retry a destructive action that was refused.
