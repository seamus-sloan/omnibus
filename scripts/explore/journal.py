#!/usr/bin/env python3
"""Append one line to a run's shared journal, atomically.

    journal.py append --run r-20260828-02 --actor agent-2 --flow reading_a_book \
        --action rating.set --target <uuid> --outcome ok \
        --params '{"old": null, "new": 4.5}'

    journal.py path --run r-20260828-02        # where that run's journal lives
    journal.py next-seq --run … --actor agent-2
    journal.py anomalies --run r-20260828-02   # what the agents flagged

Agents share one file, so `echo >> journal.jsonl` is not good enough: a shell
redirect can split a long record across two writes and a second agent's line
lands in the middle of it. This takes an exclusive lock, mints `seq` under it,
and issues the record as one write — see `audit_lib/journal.py` for why each
of those matters.

`--params` is where the *intent* goes. An entry that records the click but
not what the agent expected to happen cannot be audited or replayed, which is
how a real bug becomes an anecdote.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from audit_lib import env, journal  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--journal-dir", help="default: $OMNIBUS_EXPLORE_JOURNAL_DIR")
    sub = ap.add_subparsers(dest="cmd", required=True)

    app = sub.add_parser("append", help="append one entry")
    app.add_argument("--run", required=True)
    app.add_argument("--actor", required=True)
    app.add_argument("--action", required=True)
    app.add_argument("--flow")
    app.add_argument("--surface", default="web")
    app.add_argument("--target")
    app.add_argument("--outcome", default="ok", choices=("ok", "error", "refused", "uncertain"))
    app.add_argument("--note")
    app.add_argument("--params", default="{}", help="JSON object; '-' reads it from stdin")
    app.add_argument("--seq", type=int, help="omit to mint one under the lock")

    loc = sub.add_parser("path", help="print the journal path")
    loc.add_argument("--run", required=True)

    nxt = sub.add_parser("next-seq", help="print the next free seq for an actor")
    nxt.add_argument("--run", required=True)
    nxt.add_argument("--actor", required=True)

    ano = sub.add_parser("anomalies", help="list the anomalies agents flagged")
    ano.add_argument("--run", required=True)

    args = ap.parse_args()
    env.load()
    path = journal.journal_path(args.run, args.journal_dir)

    if args.cmd == "path":
        print(path)
        return 0
    if args.cmd == "next-seq":
        print(journal.next_seq(path, args.actor))
        return 0
    if args.cmd == "anomalies":
        # The other half of a run's story: the audit says what the server
        # lost, this says what the agents thought looked wrong.
        for e in journal.read_entries(path):
            if e.action == "anomaly":
                severity = e.params.get("severity", "?")
                print(f"{e.actor} seq={e.seq} [{severity}] {e.note or e.params.get('observed', '')}")
        return 0

    raw = sys.stdin.read() if args.params == "-" else args.params
    params = json.loads(raw)
    if not isinstance(params, dict):
        raise SystemExit("--params must be a JSON object")
    if args.outcome != "ok" and not args.note:
        raise SystemExit("--note is required whenever --outcome is not ok (start.md)")

    written = journal.append(
        path,
        {
            "run": args.run,
            "actor": args.actor,
            "surface": args.surface,
            "flow": args.flow,
            "seq": args.seq,
            "action": args.action,
            "target": args.target,
            "params": params,
            "outcome": args.outcome,
            "note": args.note,
        },
    )
    print(json.dumps({"seq": written["seq"], "ts": written["ts"]}))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except journal.JournalError as exc:
        print(f"journal: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
