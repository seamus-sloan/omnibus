---
name: agentic-exploration
argument-hint: "[--agents N] [--flows-per-agent K] [--corpus PATH] [--seed S] [--ios]"
description: Run an exploratory testing swarm against the persistent Omnibus instance — snapshot, provision accounts, sample flows from the catalog, and fan out one agent per simulated reader. Triggers when the user asks to "run the exploration swarm", "run agentic exploration", "exercise the app with agents", or invokes /agentic-exploration.
---

# Run an agentic exploration

Drives [`docs/qa/agentic_exploration/`](../../../docs/qa/agentic_exploration/start.md)
against the live instance. You are the **runner**: you own the dice, the
accounts, and the snapshot. Agents own judgement and nothing else. This mutates
a shared, long-lived instance and spends real tokens — run it when asked, not
speculatively.

## 0. Arguments — or an interview

`/agentic-exploration [--agents N] [--flows-per-agent K] [--corpus PATH] [--seed S] [--ios]`

| Arg | Default | Notes |
|---|---|---|
| `--agents` | `2` | Keep it modest; every agent is a full subagent. |
| `--flows-per-agent` | `4` | Flows are drawn **distinct** per agent. |
| `--corpus` | — | A directory of books the tests can upload. Required if `adding_book` can be drawn. |
| `--seed` | random | Print whatever you use — it is what makes a run repeatable. |
| `--ios` | off | Adds **exactly one** iOS agent on top of `--agents`. More than one is refused — see [drivers.md](drivers.md). |

**If the invocation sets none of these — no flags, and no prose fixing a value
— interview the user before doing anything else**, per
[before-run.md](before-run.md). A partial invocation is interviewed for the
rest only. That file also carries step 1.

## 1. Environment — ask for what is missing, and keep it

```bash
scripts/explore/env.sh check
```

It names every required `OMNIBUS_EXPLORE_*` setting that has no value, and
prints nothing when the run can proceed. **Ask the user for each one, then
persist it with `env.sh set <KEY> <value>`** — never invent one, and never
settle for exporting it into this shell. Wording in
[before-run.md](before-run.md).

## 2. Preflight

```bash
source scripts/explore/lib.sh && explore::load_env && explore::health
```

`200` or stop. A non-200 is the instance, not the swarm — say so and do not
snapshot, provision, or spawn anything.

## 3. Snapshot first — always

```bash
scripts/explore/snapshot.sh take pre-run
```

The database accretes forever and is never reset, so this is the only way back
from a run that corrupts something. Never skip it because a run "looks safe".
Print the name — the audit records it, and it is the rollback you hand back.

## 4. Provision accounts

```bash
scripts/explore/provision.sh <N>
```

Emits JSON: `actor`, `username`, `password`, `action`. **Save it** — the audit
needs it in steps 6 and 9, and passwords are rotated per run, never stored.
Idempotent; usernames are stable across runs because provenance ownership is
keyed on the actor, so fresh accounts would orphan every book previous runs
uploaded. Hand each agent only its own credential.

## 5. Decide the draw

```bash
explore::login_admin && explore::curl -b "$EXPLORE_JAR" "$EXPLORE_URL/api/ebooks?limit=1"
scripts/explore/sample.py --agents N --flows-per-agent K --seed S --run <run-id> \
  [--library-empty] [--exclude flow1,flow2]
```

- **`--library-empty`** when no books exist: ten flows have nothing to act on,
  so `adding_book` is forced first for every agent.
- **`--exclude`** a flow nothing can supply — no corpus makes `adding_book`
  impossible, no audio files makes `listening_to_audiobook` impossible, not
  merely blocked. Say what you excluded and why; a silent exclusion reads as
  coverage that never happened.

Weights are parsed from `flows/README.md`, the single source of truth. The
sampler exits non-zero if that table cannot be parsed or its weights do not sum
to 100 — a bug in the catalog, not a reason to sample by hand.

## 6. Set up the run

```bash
RUN=r-$(date -u +%Y%m%d)-01
JOURNAL=$(scripts/explore/journal.py path --run $RUN)
mkdir -p "$(dirname "$JOURNAL")" && : > "$JOURNAL"
```

One journal per run, shared by every agent: one timestamped timeline is what
lets the report correlate an agent's 500 with another agent's merge two seconds
earlier. It lives under `$OMNIBUS_EXPLORE_JOURNAL_DIR`, **not** under
`.claude/runtime/` — the journal is the ownership ledger, and that directory is
per-worktree, so a `wt switch` would orphan every book a previous run uploaded.
Agents append with `journal.py append`, which locks the file and mints `seq`
under that lock; a bare `>>` can split a long record in half.

Then capture the audit's baseline, so it can tell this run's writes from state
earlier runs left behind:

```bash
scripts/explore/audit.py --accounts <accounts.json> \
  capture --run $RUN --snapshot <snapshot name from step 3>
```

## 7. Start the browsers, and the simulator if `--ios`

```bash
scripts/explore/driver.sh up <N>     # one server, session and browser per agent
scripts/explore/driver.sh status
scripts/explore/driver.sh guard 1 agent-1 "$(scripts/explore/owned.sh agent-1)"
scripts/explore/ios.sh up            # `--ios` only: boot, build, install, launch
```

**One browser per agent, and at most one iOS agent.** Sharing either collapses
several users into one cookie jar, which is how run `r-20260828-01` died. Guard
every agent before any of them starts, with uuids read from the journals rather
than supplied by the agent. A browser that dies mid-run is replaced with
`driver.sh restart <n>` and guarded again. [drivers.md](drivers.md) carries the
rest: what the guard buys, how an agent drives its browser, what a dead driver
looks like, the iOS agent's extra account and scenario, and teardown.

## 8. Fan out

One subagent per actor, in parallel, each given **only**:

- the absolute path to `docs/qa/agentic_exploration/start.md`, to read in full first;
- its `actor`, its `surface` (`web`, or `ios` for the one iOS agent), the base URL, and its own username and password;
- its sampled sequence — hand over **one flow document at a time**, never the list;
- the corpus path, and `scripts/explore/journal.py append` — the only way to
  write the journal, since a bare `>>` can tear a line;
- the run id;
- **its agent number**, for `driver.sh run <n>` — never another agent's. The iOS agent gets `ios.sh` instead, which takes no agent number.

Tell each agent, verbatim in the brief: read
[`pitfalls.md`](../../../docs/qa/agentic_exploration/pitfalls.md) before
reporting anything, since most false alarms this system has produced are on
it; `uncertain` is a real verdict and beats a guessed `fail`, because a false
finding costs a human an investigation; never report a step as done that you
did not perform and observe; and every `anomaly` carries a `kind` — `defect`
when the app is wrong, `issue` when the run was — with a `note` of **at most
two short sentences**, because that note is a description cell in the report.

Ask each for: a verdict table, every anomaly with severity and the journal `seq`
to replay from, and **anything wrong or ambiguous in the flow documents
themselves** — the catalog is as much under test as the app, and that has been
the most valuable output of every run so far.

## 9. Audit the run, then report back

Follow [after-run.md](after-run.md): run `audit.py check`, generate the report
with `report.py`, then verify anything high-severity yourself before repeating
it — the difference between a finding and an anecdote has always been the check.

**Hand back the report's own three sections, in this order, on every run** —
copied from `report.md`, never from agent prose, and an empty one said to be
empty rather than dropped:

1. **Defects** — `| # | Priority | Description | Agent |`, worst first.
2. **Execution issues** — the same four columns, for friction rather than
   defects: a control that responded slowly, a step an agent could not
   validate, a step that took far longer than it should.
3. **Journal files** — every path, as bullets.

Then say what was excluded, what was left on the instance, and the snapshot
name to roll back to.

## Related

- Catalog + contract: [`docs/qa/agentic_exploration/`](../../../docs/qa/agentic_exploration/start.md)
- Journal + audit: `scripts/explore/{journal,audit}.py` (#2202) · Report: `report.py` (#2203)
- iOS lane: [`ios_lane.md`](../../../docs/qa/agentic_exploration/ios_lane.md) (#2204)
