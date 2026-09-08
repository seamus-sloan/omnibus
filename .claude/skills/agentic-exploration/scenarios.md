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
        if e.get("action") == "book.add" and e.get("outcome") == "ok":
            p = e.get("params", {})
            print(p.get("source_filename") or p.get("filename") or p.get("file") or "?")
PY
```

Hand each agent that list with the corpus path. Two agents drawing
`adding_book` in one run must be handed **different** files by you, or both
may pick the same one and the second silently attaches to the first.

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
human terms.

## The Kobo scenario (`--kobo`)

[`kobo_sync.md`](../../../docs/qa/agentic_exploration/kobo_sync.md) is
handed to **one web agent** on top of its draw, and its Parts 2 and 4 are
yours. The agent gives you the device endpoint out of band — never through the
journal — and you drive the device with the `curl` calls in that file,
journalling each under the agent's actor with `surface: kobo`. Remove the
device from the agent's account page at the end, or it persists.
