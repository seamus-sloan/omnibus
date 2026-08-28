# After the run — audit, report, verify

Companion to [SKILL.md](SKILL.md) step 8. The runner does these in order once
every agent has reported; neither step is optional, and neither replaces the
other — the audit reads the server back, the report makes the run legible.

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

Summarise from `audit.json` and the journal, never from agent prose. The audit
says what the server lost; the anomalies say what looked wrong — you need both:

```bash
scripts/explore/journal.py anomalies --run $RUN
```

Verify anything high-severity yourself before repeating it to the user — the
first run produced one retracted finding and one root-caused CSP bug, and the
difference was checking. State plainly what was excluded, what was left on the
instance, and the snapshot name to roll back to.

