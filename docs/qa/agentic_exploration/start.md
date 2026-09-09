# Agentic exploration — start here

You are one reader among several using a shared Omnibus library at the same
time. You are **not** running a test script. You are using the app, and
reporting anything that would make a real person frown.

Read this file once. Then you will be handed one flow document at a time from
[flows/](flows/); execute it, journal it, report a verdict, and wait for the
next one.

The full flow list is in [flows/README.md](flows/README.md). Every flow is
equally likely to be drawn, and you never sample from it yourself — you are
handed a flow; you execute it.

## Your identity

The harness gives you, at spawn: a **run id**, an **actor id**
(`agent-1`…`agent-N`), a **surface** (`web` or `ios`), a **base URL**, and a
**username and password**. On `ios`, read [ios_lane.md](ios_lane.md) next — it
says how to drive the app and hands you a scenario only that surface can run. Log in with those and stay logged in. Never register
a new account, and never use another agent's credentials.

Most agents are admins. That is a convenience, not a licence — see *Rails*
below. The runner may instead brief you as a **reader**: a non-admin account
that cannot delete files and sees only its own shelves, and that cannot upload
only when the runner has also turned that permission off — the brief says
which. If it does, the refusals those flows describe are yours to meet and
journal `refused`, and the criteria the catalog marks undecidable for admins —
another user's private shelf being visible, for one — become real fail
criteria for you.

## The prime directive

Behave like a person, not a crawler. Read a few pages before highlighting. Look
at a cover before opening it. Take the odd wrong turn. The value of this
exercise is entirely in the paths a spec author would never have thought to
write down, so if a flow says "browse for a while", browse for a while rather
than firing the minimum number of clicks that satisfies the wording.

Three things follow from that:

- **You navigate by clicking, never by typing.** See *Getting around* below.
  This is a rule, not a preference.
- **Flows describe intent, not selectors.** No flow document names a CSS class
  or test id, deliberately. If you cannot find the control a flow describes,
  that is a candidate finding — journal it as `uncertain` and say what you
  looked for.
- **What you type is what a person would type.** See *What you type into the
  app* below. Also a rule.

## What you type into the app

Everything you type into a field is data somebody reads later, in a shared
library that outlives the run. Shelf names, display names, metadata edits,
bookmark labels, highlight notes, journal entries — all of it persists, and
all of it is visible to every other reader.

So write what a person would write. **Never type an identifier into a field
the app displays**: no uuids, no ISO timestamps or long date strings, no run
ids, no actor ids, no counters, no `test` / `foo` / `asdf`. A shelf called
`agent-3-shelf-2026-08-26T14:02:09Z` tests nothing a reader will ever do, and
everyone else is left looking at it.

**Traceability is the journal's job, not the data's.** Every line you write
already carries `run`, `actor`, `ts`, and the values in `params` — stamping the
data as well buys nothing. Uuids still belong in the journal's `target` field,
in full; that rule is unchanged. They do not belong in anything the app renders.

Where a flow needs one value to be tellable from another — a stale copy, a
before and an after — vary it *realistically*. A display name going from
`Mara Ellison` to `Mara Ellison-Reyes` proves a stale copy exactly as well as a
counter does, and reads like a person changing their name.

Two practical limits:

- **Memorable, not unique.** Where a flow asks for something distinctive so you
  can find it again, it means a phrase you will recognise on sight — the bit
  about the lighthouse — not a generated token.
- **Do not reuse a name you have already used** for the same kind of thing. The
  audit reports two shelves sharing one name as a duplicate finding.

## Getting around

**The base URL is the only URL you ever type.** Everything else you reach by
clicking. Do not guess a path, hand-edit one, or shortcut to a page you believe
exists — an invented path is a different test, one no user runs, and the paths
agents invent are usually the ones that were never built.

The nav carries almost everything: **Library**, **Authors**, **Series**,
**Stats**, **search**, **Check in**, **Add books**, and your avatar for the
account menu, which is also where **Sign out** lives. On the web, **Check in**
is a button that opens a dialog over the page you are on, not a page of its
own. Shelves are a rail above the library on the web and a screen under
**You** on iOS; on iOS, Check in lives behind the Library masthead's `+` →
**Add books** → **Scan a barcode**. Books open from the library grid, and
everything about a book — reader, player, metadata editor, journal, saved
passages, delete — opens from that book's own page.

**Your account page is the Account section of Settings** on the web (user
menu → Edit), and the **You** tab on iOS. That one section — display name,
picture, reading goals, the detail-page scroll-stop toggle — is yours to
change; every other Settings section is on the rails below.

If you cannot find a way to reach what a flow asks for, **that is the finding**:
journal it `uncertain` and say what you looked for. If you land somewhere that
is not a page of the app, return to the base URL and start again from the nav —
never repair a path by hand.

## Ownership — you may only destroy what you made

Anyone may read anything, and anyone may edit metadata, genres, tags, and
covers on any book. But these actions are **owner-only**:

- deleting a book or one of its files
- merging books
- removing a physical copy filed against a book
- unmerging (allowed only to reverse a merge you just made)

You own a book if **you added it** — in this run or any earlier one, by
uploading it or by checking in a paper-only copy. The journal is the ownership
ledger: you own uuid X if a `book.add` entry with `actor` equal to you and
`target` equal to X exists in any run's journal. Journals are kept forever
next to the instance for exactly this reason.

The baseline corpus was added by nobody, so **nobody may ever destroy it**.

The server will not enforce any of this when you are an admin — but your
browser will. Destructive calls to a book you do not own are refused before
they are sent, and you will see a `403` carrying `"error": "ownership_guard"`.

**That refusal is correct behaviour, not a bug and not an obstacle.** Journal it
`refused`, do not retry it, and do not go looking for another route to the same
act. If a flow document appears to require one, the document is wrong — journal
an `anomaly` against it.

## The journal

Everything you do goes in the shared journal — one JSON object per line,
appended, never rewritten. It is the only durable record of the run; agent
transcripts are thrown away.

```json
{"ts":"2026-08-26T14:02:09.412Z","run":"r-2026-08-26-01","actor":"agent-3",
 "surface":"web","flow":"adding_highlight","seq":7,
 "action":"highlight.create","target":"9f2c1e4a-7b60-4d33-9a2f-1c85e0d47b12",
 "params":{"format":"epub","location":"chapter 4, para 12","colour":"green",
 "note_text":"check this against the appendix"},
 "outcome":"ok","note":null}
```

| Field | Meaning |
|---|---|
| `ts` | UTC, ms precision. The report correlates agents on this alone — never batch entries and stamp them later. |
| `seq` | Your own counter, monotonic and unique **per actor**. Derive it from the journal filtered to your own `actor`, never from the line count — the journal is shared, so counting all lines numbers you by other agents' work. Starting above 1 is acceptable; going backwards or repeating is not. |
| `surface` | `web` or `ios`, as briefed. The runner writes `phantom` or `kobo` on entries it makes on your behalf; never use those yourself. |
| `action` | Dotted `noun.verb` — `book.open`, `highlight.create`, `metadata.save`, `shelf.add`. Use the names the flow document lists. The report matches names as strings; the audit classifies them by noun and verb, and every name a flow document lists is classified — as a write it checks, a look, or something it declines by policy — while an invented one lands in `unverifiable` as a gap. A trailing `.verify` on any name means "I checked it stuck" and is always accepted. |
| `target` | The book uuid or other entity id, **in full** — never abbreviated. Ownership is looked up on this exact string, so a truncated uuid loses the book forever. `null` when there isn't one. |
| `params` | **Everything a replayer needs to redo this.** Under-filling it is the commonest way a real bug becomes an anecdote. |
| `outcome` | `ok`, `error`, `refused` (an ownership or permission refusal that was correct), or `uncertain` (you did it and cannot tell whether it took). Anything but `ok` needs a `note`, and the audit does not check a write that is not `ok`. |
| `note` | One human sentence **about the outcome**. Required whenever `outcome` is not `ok`. Content the *user* wrote — a highlight's note, a journal entry — belongs in `params` under its own key (`note_text`), never here. |

Three entries are special, and a subflow that runs inside another flow
gets its **own** pair of them — `adding_highlight` inside `reading_a_book`
opens and closes itself, under its own `flow` name, between the parent's
start and end:

- **`flow.start`** — first line of every flow. `params` carries `base_url`,
  the instance you are driving; the report names the instance from it.
- **`anomaly`** — something looked wrong. `params` carries `severity`
  (`high`/`medium`/`low`), `expected`, `observed`, and `kind`: **`defect`** when
  the app is wrong, **`issue`** when the *run* was — a control that responded
  slowly, a step you could not validate, a step that took far longer than it
  should. The two are reported in separate tables, and an anomaly with no
  `kind` is read as a defect. Keep its `note` to **two short sentences at
  most**: that note is a description cell in the report, and the detail belongs
  in `expected`, `observed`, and `repro`. Four more keys are optional and the
  report expands each under its own label when present: `repro` (the steps to
  see it again), `where` (the page or control), `impact` (why a reader would
  care), and `caveat` (what you checked and ruled out).
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

## Time

A flow is a lunch break, not an afternoon. Budget **twenty minutes** for one,
thirty for a reading or listening flow, and treat a single control that has
not answered in **thirty seconds** as hung — that is a `fail` criterion above,
not a reason to keep waiting. A step you cannot finish inside a few minutes
more than it should take gets an `anomaly` of kind `issue` with how long it
took, and the flow moves on.

When the budget runs out, end the flow `uncertain` with the step you reached
in `reason`, journal what you did finish, and take the next one. A flow that
runs an hour tells the runner less than one that stopped and said why.

## Before you report anything: read pitfalls.md

[pitfalls.md](pitfalls.md) lists what looks like a defect and is not — both
deliberate app behaviour and the ways your own browser lies to you. Read it
before your first flow. Nearly every false alarm this system has produced was
already on that list.

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
- Touch **Settings** — library paths, API keys, SMTP, users, and the like
  are instance-wide configuration and one edit breaks the run for everyone.
  The **Account** section is the one exception, and only for what *Your
  identity* above lists.
- Trigger a reindex, a library scan, or an FTS rebuild.
- Send to Kindle or Kobo. These deliver real things to real places. (Marking a
  shelf for Kobo *sync* is different — it sends nothing until a device asks —
  and only the agent handed [kobo_sync.md](kobo_sync.md) does that.)
- Change another user's account or permissions.
- Delete a user.
- Change your own password or Kindle email. Both live beside your profile;
  a changed password locks you out of the rest of the run.
- Register an account.
- Delete an author or a series. These are library-wide and derived, and no
  agent owns them — the guard refuses the call outright, whatever you added.

If a flow appears to ask for one of these, stop and journal an `anomaly` about
the flow document. The document is wrong, not you.

## When you are stuck

Journal what you saw, end the flow `uncertain`, and take the next one. Do not
improvise a recovery that puts the library in a state nobody can explain
afterwards, and do not retry a destructive action that was refused.

## Where the corpus is

Several flows ask for a file from the corpus — `adding_book` for a book to
upload, `adding_journal` and `updating_profile` for a `cover.jpg` sidecar to
use as an image. **The corpus root is whatever the runner names in your brief,
and the brief must name it**; two agents in run r-20260908-02 had to recover
the path by reading other agents' journals because no document stated it. If
your brief does not give you a corpus path and a flow asks for one, say so and
journal the step `uncertain` rather than going to look for one.
