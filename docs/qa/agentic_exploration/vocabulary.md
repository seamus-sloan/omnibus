# Action vocabulary

Every journal entry carries an `action`. The audit reads those names to decide
what it should go and check on the server, so a name it does not recognise is a
write it cannot verify.

Two free-running runs produced **52 distinct names across 197 entries, 27 of
them used exactly once** — nine verbs on `player`, six on `book`, six on
`reader`. `book.open`, `book.view` and `book.detail.open` were three names for
one act. An audit keyed on exact strings would have skipped every name it did
not know, and reported success while doing it.

So the vocabulary is **a closed list of nouns with a conventioned verb slot**,
not a closed list of names. `scripts/explore/audit_lib/vocabulary.py` is the
source of truth; this page is the agent-facing summary of it.

## The shape of a name

`noun.verb`, lowercase. Separators `.`, `_` and `-` are equivalent, so
`reader.resume_check` and `reader.resume.check` are the same thing — spelling
drift cannot cost you coverage.

**A trailing qualifier means "I looked" on any noun.** `verify`, `check`,
`recheck`, `attempt`, `view`, `read`, `inspect`, `observe` and friends. This
exists because agents kept inventing `book.add.verify` and
`journal.persist.verify`: they needed a way to say *I checked it stuck* and the
contract offered none. Now it is answered by rule.

The consequence to know: **journal the bare `noun.verb` for the write itself,
and the qualified form only for the re-check.** `rating.set.attempt` on its own
reads as an observation and gets no audit coverage at all.

## The three noun policies

| Policy | Nouns | An unlisted verb means |
|---|---|---|
| **Audited state** | `rating` `status` `progress` `reader` `player` `journal` `highlight` `bookmark` `shelf` `wishlist` `book` | `unknown` — named in the report, never silently dropped |
| **No audited state** | `nav` `ui` `search` `library` `author` `series` `stats` `suggestions` `auth` `session` `flow` `page` | an observation; nothing there to miss |
| **Out of scope** | `metadata` `cover` `genre` `tag` `merge` `profile` `settings` `checkin` `kindle` `kobo` | out of scope, with the reason quoted into the report |

Only the first row is strict, and only because the verb is what tells the audit
whether to assert a value is *present* or *absent* — guessing that backwards
invents findings. An unlisted verb there lands in the report's `unverifiable`
list **naming the action**, so an invented verb is a visible gap rather than
silent coverage.

Plurals and spelling variants are folded for you: `ratings` → `rating`,
`annotations` → `highlight`, `listen`/`audio`/`playback` → `player`,
`reading` → `reader`.

## Reserved names

`flow.start`, `flow.end` and `anomaly` are exact and structural. Do not
decorate them.

## When your verb isn't listed

Use it anyway and journal what you did. It surfaces as a named gap rather than
vanishing, which is the whole design — and adding it costs one line in
`vocabulary.py`. `audit.py vocab --run <id>` lists what a run invented.

Never bend an action name to fit a verb you think the audit wants. A name that
misdescribes what you did is worse than one the audit has to flag.
