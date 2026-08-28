---
name: agentic-exploration
description: Run an exploratory testing swarm against the persistent Omnibus instance — snapshot, provision accounts, sample flows from the catalog, and fan out one agent per simulated reader. Triggers when the user asks to "run the exploration swarm", "run agentic exploration", "exercise the app with agents", or invokes /agentic-exploration.
---

# Run an agentic exploration

Drives [`docs/qa/agentic_exploration/`](../../../docs/qa/agentic_exploration/start.md)
against the live instance. You are the **runner**: you own the dice, the
accounts, and the snapshot. Agents own judgement and nothing else.

> This mutates a shared, long-lived instance and spends real tokens. Run it when
> asked, not speculatively.

## 0. Arguments

`/agentic-exploration [--agents N] [--flows-per-agent K] [--corpus PATH] [--seed S] [--ios]`

| Arg | Default | Notes |
|---|---|---|
| `--agents` | 2 | Keep it modest; every agent is a full subagent. |
| `--flows-per-agent` | 4 | Flows are drawn **distinct** per agent. |
| `--corpus` | — | Directory of real book files. Required if `adding_book` can be drawn. |
| `--seed` | random | Print whatever you use — it is what makes a run repeatable. |
| `--ios` | off | **Not implemented.** Refuse and point at #2204 rather than silently running web-only. |

## 1. Preflight

```bash
source scripts/explore/lib.sh && explore::load_env && explore::health
```

`200` or stop. A non-200 is the instance, not the swarm — say so and do not
snapshot, provision, or spawn anything.

`OMNIBUS_EXPLORE_URL` and `OMNIBUS_EXPLORE_ADMIN` come from the gitignored
`.env`. That admin credential is the **only** secret this system persists; if it
is missing, ask for it rather than inventing accounts.

## 2. Snapshot first — always

```bash
scripts/explore/snapshot.sh take pre-run
```

The database accretes forever and is never reset, so this is the only way back
from a run that corrupts something. Never skip it because a run "looks safe".
Print the name — the audit records it, and it is the rollback you hand back.

## 3. Provision accounts

```bash
scripts/explore/provision.sh <N>
```

Emits JSON: `actor`, `username`, `password`, `action`. **Save it** — the audit
needs it in steps 5 and 8, and passwords are rotated per run, never stored.
Idempotent; usernames are stable across runs because provenance ownership is
keyed on the actor, so fresh accounts would orphan every book previous runs
uploaded. Hand each agent only its own credential.

## 4. Decide the draw

```bash
explore::login_admin && explore::curl -b "$EXPLORE_JAR" "$EXPLORE_URL/api/ebooks?limit=1"
scripts/explore/sample.py --agents N --flows-per-agent K --seed S --run <run-id> \
  [--library-empty] [--exclude flow1,flow2]
```

- **`--library-empty`** when no books exist: ten flows have nothing to act on,
  so `adding_book` is forced first for every agent.
- **`--exclude`** a flow the corpus cannot supply — no audio files makes
  `listening_to_audiobook` impossible, not merely blocked. Say what you
  excluded and why; a silent exclusion reads as coverage that never happened.

Weights are parsed from `flows/README.md`, the single source of truth. The
sampler exits non-zero if that table cannot be parsed or its weights do not sum
to 100 — a bug in the catalog, not a reason to sample by hand.

## 5. Set up the run

```bash
RUN=r-$(date -u +%Y%m%d)-01
JOURNAL=$(scripts/explore/journal.py path --run $RUN)
mkdir -p "$(dirname "$JOURNAL")" && : > "$JOURNAL"
```

One journal per run, shared by every agent: one timestamped timeline is what
lets the report correlate an agent's 500 with another agent's merge two
seconds earlier. It lives under `$OMNIBUS_EXPLORE_JOURNAL_DIR`, **not** under
`.claude/runtime/`: the journal is the ownership ledger and
`.claude/runtime/` is per-worktree, so keeping it in the repo means a
`wt switch` orphans every book a previous run uploaded. Agents append with
`scripts/explore/journal.py append`, which locks the file and mints `seq`
under that lock; a bare `>>` can split a long record in half.

Then capture the audit's baseline, so it can tell this run's writes from state
earlier runs left behind:

```bash
scripts/explore/audit.py --accounts <accounts.json> \
  capture --run $RUN --snapshot <snapshot name from step 2>
```

## 6. Start the browsers

```bash
scripts/explore/driver.sh up <N>      # one server, session and browser per agent
scripts/explore/driver.sh status
```

**One browser per agent is not a nicety.** Run `r-20260828-01` died because
three subagents shared a tab: one cookie jar, three users collapsed into one,
and two agents correctly aborted rather than journal under a wrong actor.

Agents drive their browser with `driver.sh run <n> "<command>"`, which prints
`{"text": ..., "isError": ...}`. Tear down with `driver.sh down` when the run
ends, whatever the outcome.

Then **guard each agent** before any of them starts:

```bash
scripts/explore/driver.sh guard <n> agent-<n> <comma-separated-uuids>
```

The uuids come from the journals, never from the agent:

```bash
scripts/explore/driver.sh guard 1 agent-1 "$(scripts/explore/owned.sh agent-1)"
```

`owned.sh` reads **every** journal, not just this run's — ownership is
provenance and durable.

Without this, ownership is only a sentence in `start.md`, and every exploration
account is an admin — so nothing stops one agent destroying another's books.

After the run, `driver.sh refusals <n>` lists what each agent was stopped from
doing. **A non-empty list is a finding about the agent or the flow document,
not about the app.**

## 7. Fan out

One subagent per actor, in parallel, each given **only**:

- the absolute path to `docs/qa/agentic_exploration/start.md`, to read in full first;
- its `actor`, `surface: web`, the base URL, and its own username and password;
- its sampled sequence — hand over **one flow document at a time**, never the list;
- the corpus path, and `scripts/explore/journal.py append` — the only way to
  write the journal, since a bare `>>` can tear a line;
- the run id;
- **its agent number**, for `driver.sh run <n>` — never another agent's.

Tell each agent, verbatim in the brief: read
[`pitfalls.md`](../../../docs/qa/agentic_exploration/pitfalls.md) before
reporting anything, since most false alarms this system has produced are on
it; `uncertain` is a real verdict and beats a guessed `fail`, because a false
finding costs a human an investigation; and never report a step as done that
you did not perform and observe.

Ask each for: a verdict table, every anomaly with severity and the journal `seq`
to replay from, and **anything wrong or ambiguous in the flow documents
themselves** — the catalog is as much under test as the app, and that has been
the most valuable output of every run so far.

## 8. Audit the run, then report back

Agent prose is unverified; the audit is the only thing that reads the server
back.

```bash
scripts/explore/audit.py --accounts <accounts.json> check --run $RUN
scripts/explore/audit.py vocab --run $RUN     # action names nobody taught it
```

`check` writes `audit.json` next to the journal. Read three things from it:
**`findings`** (`missing`/`mismatch`/`unexpected`/`duplicate`, each carrying
`replay_from` — the `seq` for `audit.py replay --actor <a> --from <seq>`);
**`unverifiable`**, what it declined to judge and why — an *unrecognised
action* there is a verb an agent invented, so add its `(noun, verb)` row to
`audit_lib/vocabulary.py` in the same session; and **`checked`** — many writes
and few checks means the journals are under-filled, not that the app is healthy.

Summarise from `audit.json` and the journal, never from agent prose. The audit
says what the server lost; the anomalies say what looked wrong — you need both:

```bash
scripts/explore/journal.py anomalies --run $RUN
```

Verify anything high-severity yourself before repeating it to the user — the
first run produced one retracted finding and one root-caused CSP bug, and the
difference was checking. State plainly what was excluded, what was left on the
instance, and the snapshot name to roll back to.

## Related

- Catalog + contract: [`docs/qa/agentic_exploration/`](../../../docs/qa/agentic_exploration/start.md)
- Journal + audit: `scripts/explore/{journal,audit}.py` (#2202) · Report: #2203 · iOS: #2204
