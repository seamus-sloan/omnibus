# Runner-driven scenarios and hand-edits to the draw

Companion to [SKILL.md](SKILL.md) steps 4, 5 and 8. Three things the sampler
and the scripts do not do for you, and how to do them by hand without turning
a guess into coverage.

## Editing the draw after `sample.py`

The sampler draws top-level flows and rolls their subflows. It cannot exclude
a subflow, and it cannot vary the draw per agent. Two edits are routine, and
**every edit is said in the hand-back** — a silent edit reads as a flow that
ran.

- **`merging_books` needs two owned books.** It always runs inside
  `adding_book`, which uploads one — so the agent needs at least one more
  from an earlier run. Before the run, count each agent's owned uuids:
  `scripts/explore/owned.sh agent-N | tr ',' '\n' | wc -l`. An agent with
  none cannot merge; strip the subflow from that agent's sequence.
- **The iOS agent never runs `adding_book`, `merging_books` or
  `deleting_a_book`** — there is no ownership guard on that surface. Re-draw
  its sequence with `--exclude adding_book` and a fresh `--agents 1`, using
  the same seed plus one so the run stays reproducible, and say so.
- **A comic reader needs a CBZ, and a listening flow needs audio.** The
  library listing's formats column says which exist. If none does, `--exclude
  listening_to_audiobook` at draw time; for comics, tell the agent handed
  `reading_a_book` that no CBZ exists so it does not hunt for one.
- **Order `viewing_stats` late.** Stats read back what earlier flows wrote,
  so when an agent draws it first, move it after its first reading or
  listening flow. Say so.

## The corpus files already used

`adding_book.md` promises the agent a list of corpus files already uploaded.
Derive it from every journal — uploads in earlier runs count:

```bash
python3 - "$OMNIBUS_EXPLORE_JOURNAL_DIR" <<'PY'
import json, pathlib, sys
for j in sorted(pathlib.Path(sys.argv[1]).glob("*/journal.jsonl")):
    for line in j.open():
        e = json.loads(line)
        # Require a target: a `book.add` whose upload never produced a
        # book (r-20260829-01's crashed audiobook) must not retire its file.
        if e.get("action") == "book.add" and e.get("outcome") == "ok" and e.get("target"):
            p = e.get("params", {})
            print(p.get("source_filename") or p.get("filename") or p.get("file") or "?")
PY
```

Hand each agent that list with the corpus path. Two agents drawing
`adding_book` in one run must be handed **different** files by you, or both
may pick the same one and the second silently attaches to the first.

## Re-guard after every upload, and hand the destructive subflows separately

`driver.sh guard` bakes the owned set into the browser when it runs, and it
never re-reads the journal. **This is not only `adding_book`'s problem**: any
flow that mints a book mid-run hits it. A `checking_in_a_book` lookup that
finds nothing creates a paper-only book, and the agent is then refused the
delete of its *own* book — which leaves the copy-less row #2497 is about. If a
flow can create a book, either split its hand-over the same way or re-guard as
soon as the agent journals a `book.add`. A book uploaded mid-run is therefore **not** in
its own agent's owned set, and `merging_books` and `deleting_a_book` — which
always follow `adding_book` — are refused for exactly the book they exist to
act on. So the hand-over is three steps, not one:

1. hand `adding_book` alone, and wait for its report;
2. `driver.sh guard <n> agent-<n> "$(scripts/explore/owned.sh agent-<n>)"`,
   which now includes the new uuid;
3. hand `merging_books` and `deleting_a_book` as their own step.

Say in the brief that the subflows will follow separately, so the agent does
not start their `flow.start` lines early.

## One fresh subagent per flow

A subagent cannot be messaged after it reports, so every flow is a fresh
subagent. Keep the standing part of the brief — identity, driver, journal,
rails, corpus — in one file per agent and point each new subagent at it, and
give it a two-sentence recap of what its actor did in earlier flows (the
books it touched, the names it used, where the browser is). Each brief tells
the agent to use a scratch directory named for its actor: the harness's
scratchpad is shared, and one run routed an agent's commands into another
agent's browser through a clobbered helper script.

Two things look like a finished agent and are not. A subagent that backgrounds
a long sleep fires a completion notification while its flow is still open —
**read the journal for a `flow.end` before handing the next flow**; the
duplicate iOS agent of run `r-20260908-01` came from trusting the
notification. And an API rate limit kills every subagent at once, mid-flow:
on resume, derive each actor's state from the journal (open flows are a
`flow.start` with no `flow.end`) and brief the fresh subagent to finish them
without writing a second `flow.start`.

## Non-admin readers

Every provisioned account is an admin. `provision.sh` creates non-admins only
for a whole call, so mixing the two takes two calls, and only accounts
created in the second one are non-admin — an existing account keeps whatever
permissions it was created with:

```bash
scripts/explore/provision.sh <N>              # explorer-1..N, admins as before
scripts/explore/provision.sh <N+1> --no-admin # creates explorer-(N+1) non-admin;
                                              # 1..N only have their passwords rotated
```

Give the extra account to one agent as a **reader**, and tell it so in the
brief: it will see no **Delete files…** and no other user's private shelves —
the criteria the catalog marks undecidable for admins become decidable for
it. `provision.sh` always grants upload, so the "You don't have permission to
add books" refusal needs you to turn that permission off yourself, as the
admin in **Settings → Users** on the instance, before the run — and to say in
the hand-back that you did, since a later `provision.sh` call leaves it as
you set it.

## The phantom device (`resuming_from_another_device`)

Before handing that subflow over, write a position to the agent's own account
as a second device would. `$ACCT` is the agent's `username:password` from
`provision.sh`, `$JAR` a fresh cookie jar:

```bash
source scripts/explore/lib.sh && explore::load_env
explore::curl -c "$JAR" -X POST "$EXPLORE_URL/api/auth/login" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"${ACCT%%:*}\",\"password\":\"${ACCT#*:}\"}" -o /dev/null
explore::curl -b "$JAR" -X POST "$EXPLORE_URL/api/progress" \
  -H 'Content-Type: application/json' \
  -d '{"book_uuid":"<uuid>","format":"epub","progress_percent":40,
       "client_updated_at":<unix seconds>}'
```

- **`newer`**: a percent ahead of anything the agent has reached, and
  `client_updated_at` = now. Optionally also `PUT /api/read-status` with
  `{"book_uuid":…,"status":"reading"}`, and say that you did.
- **`stale`**: only on a book the agent has already read in an earlier flow.
  A percent behind its last position, and `client_updated_at` a few minutes
  *before* that position's timestamp — the server keeps the newest event.

Journal it under the agent's actor with `journal.py append --actor agent-N
--surface phantom --flow resuming_from_another_device --action progress.set
--target <uuid> --params '{"format":"epub","percent":40,"variant":"newer"}'`.
Then hand the agent the flow with the uuid, the variant, and the position in
human terms. Pick a book no other agent is reading or editing, and one the
agent has not opened — the audit keys progress per book, and a percent
placed ahead of a later sitting makes that sitting credit zero pages on the
Stats page, which is honest but confusing to the agent handed `viewing_stats`
afterwards.

## The Kobo scenario (`--kobo`)

[`kobo_sync.md`](../../../docs/qa/agentic_exploration/kobo_sync.md) is
handed to **one web agent** on top of its draw, and its Parts 2 and 4 are
yours. The agent gives you the device endpoint out of band — never through the
journal — and you drive the device with the `curl` calls in that file,
journalling each under the agent's actor with `surface: kobo`. Remove the
device from the agent's account page at the end, or it persists.
