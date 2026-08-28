# Before the run — the interview, and the environment

Companion to [SKILL.md](SKILL.md) steps 0 and 1. Both exist for the same
reason: a run started on a guessed value is worse than one that asked, because
nothing downstream can tell a guess from an answer.

## The interview

Trigger: the invocation set **none** of the arguments in SKILL.md's table.
"Set" includes prose as well as flags — *run four agents*, *use the books in
~/corpus*, *skip the phone* each fix a value as firmly as `--agents 4` does. A
partial invocation is interviewed for the rest only; re-asking something the
user already said is how an interview becomes a toll.

Ask with `AskUserQuestion`, one question per unset argument, and **offer the
default from SKILL.md's table as an option, labelled as the default** — a
question with no default asks the user to guess at a knob they came here to
avoid. The numeric defaults are `sample.py`'s own, so the two cannot drift.

| Unset | Ask | Options |
|---|---|---|
| `--agents` | How many agents should run? | `2` (default), 1, 4 |
| `--flows-per-agent` | How many flows should each agent run? | `4` (default), 2, 6 |
| `--corpus` | Where is the corpus — **a directory of books the tests can upload**? | a path, or none |
| `--seed` | Reuse a seed, or draw a fresh one? | fresh (default), or a number |
| `--ios` | Add the one iOS agent? | no (default), yes |

The corpus question says what a corpus *is* because the word is this system's
alone: a reader who has not read `start.md` cannot answer "where is the
corpus", and will answer "a directory of books the tests can upload".

If the user gives no corpus, `adding_book` cannot run: `--exclude` it at step 5
and say you did. Never substitute a directory you found yourself — the corpus
is what the run uploads into a real library, so choosing it is the user's.

## The environment

```bash
scripts/explore/env.sh check
```

It names every required `OMNIBUS_EXPLORE_*` setting that has no value, one per
line, and prints nothing when the run can proceed. **Ask the user for each one
it names, then persist it.** Never invent a value, and never settle for
exporting it into this shell: the next run is a different session, and a value
that lived only in this one gets asked for again.

| Missing | Ask for | Then |
|---|---|---|
| `OMNIBUS_EXPLORE_URL` | The instance to explore. Say plainly that agents upload, edit, merge and delete on it, so it must be a throwaway — never a library the user cares about. | `env.sh set OMNIBUS_EXPLORE_URL <url>` |
| `OMNIBUS_EXPLORE_ADMIN` | An admin `user:password` on that instance — the only secret this system persists. | `env.sh set OMNIBUS_EXPLORE_ADMIN <user:password>` |
| `OMNIBUS_EXPLORE_JOURNAL_DIR` | Where journals live: a path **outside** the worktrees. Offer `$HOME/.omnibus-explore/journals`. | `env.sh set OMNIBUS_EXPLORE_JOURNAL_DIR <path>` |

`OMNIBUS_EXPLORE_JOURNAL_DIR` is asked for like the other two even though
`owned.sh` has a fallback, because that fallback is `.claude/runtime/` —
gitignored and per-worktree — so accepting it means the next `wt switch`
orphans every book earlier runs uploaded.

`set` writes into the repo's gitignored `.env`, replacing the key in place —
commented or live — and dropping any duplicate: `lib.sh` reads the first
occurrence and `audit_lib.env` the last, so a file carrying two would let the
shell and Python halves of one run target different instances. It refuses a key
nothing reads and a value that would not survive being loaded back.

The optional settings (`OMNIBUS_EXPLORE_SSH_HOST` and its siblings) are never
prompted for. Snapshotting and the server-log half of the report need them; a
run without them still works, and says what it could not read.
