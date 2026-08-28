---
name: agentic-exploration
description: Run an exploratory testing swarm against the persistent Omnibus instance — snapshot, provision accounts, sample flows from the catalog, and fan out one agent per simulated reader. Triggers when the user asks to "run the exploration swarm", "run agentic exploration", "exercise the app with agents", or invokes /agentic-exploration.
---

# Run an agentic exploration

Drives [`docs/qa/agentic_exploration/`](../../../docs/qa/agentic_exploration/start.md)
against the live instance. You are the **runner**: you own the dice, the
accounts, and the snapshot. Agents own judgement and nothing else. This mutates
a shared, long-lived instance and spends real tokens — run it when asked, not
speculatively.

## 0. Arguments

`/agentic-exploration [--agents N] [--flows-per-agent K] [--corpus PATH] [--seed S] [--ios]`

| Arg | Default | Notes |
|---|---|---|
| `--agents` | 2 | Keep it modest; every agent is a full subagent. |
| `--flows-per-agent` | 4 | Flows are drawn **distinct** per agent. |
| `--corpus` | — | Directory of real book files. Required if `adding_book` can be drawn. |
| `--seed` | random | Print whatever you use — it is what makes a run repeatable. |
| `--ios` | off | Adds **exactly one** iOS agent on top of `--agents`. More than one is refused — see step 6a. |

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
from a run that corrupts something, and it is the baseline the audit (#2202)
will diff against. Never skip it because a run "looks safe". Print the name.

## 3. Provision accounts

```bash
scripts/explore/provision.sh <N>
```

Emits JSON: `actor`, `username`, `password`, `action`. Idempotent — usernames
are **stable across runs** because provenance ownership is keyed on the actor,
so a fresh account each run would orphan every book previous runs uploaded.
Passwords are rotated per run and never stored.

Hand each agent only its own credential.

## 4. Decide the draw

Check whether the library has anything in it:

```bash
explore::login_admin && explore::curl -b "$EXPLORE_JAR" "$EXPLORE_URL/api/ebooks?limit=1"
```

Then sample:

```bash
scripts/explore/sample.py --agents N --flows-per-agent K --seed S --run <run-id> \
  [--library-empty] [--exclude flow1,flow2]
```

- **`--library-empty`** when no books exist: ten of the flows have nothing to
  act on, so `adding_book` is forced first for every agent.
- **`--exclude`** a flow the corpus cannot supply. A corpus with no audio files
  makes `listening_to_audiobook` structurally impossible, not merely blocked —
  handing it over wastes a slot. Say in the report what you excluded and why;
  a silent exclusion reads as coverage that never happened.

Weights are parsed from the catalog table in `flows/README.md`, which is the
single source of truth. The sampler exits non-zero if that table cannot be
parsed or its top-level weights do not sum to 100 — treat that as a bug in the
catalog, not a reason to sample by hand.

## 5. Set up the run

```bash
RUN=r-$(date -u +%Y%m%d)-01
mkdir -p .claude/runtime/explore/$RUN && : > .claude/runtime/explore/$RUN/journal.jsonl
```

One journal per run, shared by every agent. Six transcripts is something nobody
reads; one timestamped timeline is what lets the report correlate an agent's
500 with another's merge two seconds earlier.

## 6. Start the browsers

```bash
scripts/explore/driver.sh up <N>      # one server, session and browser per agent
scripts/explore/driver.sh status
```

**One browser per agent is not a nicety.** Run `r-20260828-01` died because
three subagents shared a single browser tab: one cookie jar, three users
collapsed into one, and two agents correctly aborted rather than journal under
a wrong actor. The driver makes that impossible.

Agents drive their browser with `driver.sh run <n> "<command>"`, which prints
`{"text": ..., "isError": ...}`. Tear down with `driver.sh down` (and
`ios.sh down`) when the run ends, whatever the outcome.

Then **guard each agent** before any of them starts. The uuids come from the
journals, never from the agent:

```bash
scripts/explore/driver.sh guard 1 agent-1 "$(scripts/explore/owned.sh agent-1)"
```

`owned.sh` reads **every** journal, not just this run's — ownership is
provenance and is durable, the same reason `provision.sh` keeps usernames
stable. Without the guard, ownership is only a sentence in `start.md` and every
exploration account is an admin, so nothing stops one agent destroying
another's books; with it, the request is refused before it is sent.

After the run, `driver.sh refusals <n>` lists what each agent was stopped from
doing. **A non-empty list is a finding about the agent or the flow document,
not about the app** — report it as such.

### 6a. Start the iOS agent, if `--ios`

```bash
scripts/explore/ios.sh up            # boot a simulator, build, install, launch
scripts/explore/ios.sh state         # {online, running, forced_offline}
```

**One iOS agent, ever.** Two on one simulator share a keychain, a container and
a session — the same collapse, with no isolation available to fix it. `--ios`
adds one; asking for more is a refusal, not a clamp.

It is a full agent with its own account, so provision `N+1`. Give it
`surface: ios`, [`ios_lane.md`](../../../docs/qa/agentic_exploration/ios_lane.md)
alongside `start.md`, and the `offline_outbox` scenario from that file **on top
of** its sampled flows — it is the only surface that can run it. There is no
ownership guard here: keep it off `adding_book` and `merging_books`.

## 7. Fan out

One subagent per actor, in parallel, each given **only**:

- the absolute path to `docs/qa/agentic_exploration/start.md`, to read in full first;
- its `actor`, its `surface` (`web`, or `ios` for the one iOS agent), the base URL, and its own username and password;
- its sampled sequence — hand over **one flow document at a time**, never the list;
- the corpus path, and the journal path;
- the run id;
- **its agent number**, for `driver.sh run <n>` — never another agent's. The iOS agent gets `ios.sh` instead, which takes no agent number.

Tell each agent, verbatim in the brief:

- Read [`pitfalls.md`](../../../docs/qa/agentic_exploration/pitfalls.md) before
  reporting anything. Most false alarms this system has produced are on it.
- `uncertain` is a real verdict. An honest `uncertain` beats a guessed `fail`,
  because a false finding costs a human an investigation.
- Never report a step as done that it did not perform and observe.

Ask each for: a verdict table, every anomaly with severity and the journal `seq`
to replay from, and **anything wrong or ambiguous in the flow documents
themselves**. That last one has been the most valuable output of every run so
far — the catalog is as much under test as the app.

## 8. Report back

The report is generated, not written by hand — agent prose is unverified, and
the journal plus the server log are the only records that are not.

```bash
python3 scripts/explore/report.py $RUN          # -> <journal dir>/$RUN/report.md
python3 scripts/explore/report.py $RUN --out -  # to stdout
```

It reads the run's `journal.jsonl`, the `audit.json` beside it, and the
instance's JSON log sink over ssh, and emits one markdown document: verdict,
severity-ranked anomalies each citing their replay line, server-log findings
joined to the causing agent action, the audit's unconfirmed writes, a collapsed
timeline. Empty sections are omitted — but an input it could not read is always
named in the verdict rather than passing as clean.

Instance unreachable? `--no-server-log` skips the fetch, `--server-log <file>`
reads one you have; `--window` widens correlation (default 90s).

Then verify anything high-severity yourself before repeating it to the user —
the difference between a finding and an anecdote has always been the check.

State plainly what was excluded, what was left on the instance, and the snapshot
name to roll back to.

## Related

- Catalog + contract: [`docs/qa/agentic_exploration/`](../../../docs/qa/agentic_exploration/start.md)
- iOS lane: [`ios_lane.md`](../../../docs/qa/agentic_exploration/ios_lane.md)
- Journal + audit: #2202 · Report: #2203
