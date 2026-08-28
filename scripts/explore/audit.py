#!/usr/bin/env python3
"""Reconcile what an exploration run's agents said they did against what the
instance actually holds.

    audit.py capture --run <id> --accounts <file>   # baseline, before the run
    audit.py check   --run <id> --accounts <file>   # findings, after it
    audit.py replay  --run <id> --actor agent-2 --from 27
    audit.py vocab   --run <id>                     # action-name coverage

`check` writes `audit.json` next to the journal — the contract the run report
consumes. It never invents its own shape: `findings` carry one of `missing`,
`mismatch`, `unexpected` or `duplicate`, and everything the audit declined to
judge is listed in `unverifiable` with a reason rather than dropped.

Per-user state is only readable as that user, so `--accounts` takes the JSON
`provision.sh` emits — the credentials the run already minted. Nothing here
persists a password.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from audit_lib import compare, env, expectations, journal, replay, state, vocabulary  # noqa: E402
from audit_lib.client import Account, ApiError, load_accounts  # noqa: E402


def resolve_accounts(args: argparse.Namespace) -> dict[str, Account]:
    """Load `provision.sh`'s JSON from a file or stdin."""
    if not args.accounts:
        raise SystemExit(
            "--accounts is required: pass the JSON provision.sh emitted for this run "
            "(or '-' to read it on stdin). Per-user state is only readable as that user."
        )
    raw = sys.stdin.read() if args.accounts == "-" else Path(args.accounts).read_text(encoding="utf-8")
    return load_accounts(json.loads(raw))


def base_url(args: argparse.Namespace) -> str:
    url = args.url or os.environ.get("OMNIBUS_EXPLORE_URL")
    if not url:
        raise SystemExit("no instance URL: pass --url or set OMNIBUS_EXPLORE_URL in .env")
    return url.rstrip("/")


def journal_file(args: argparse.Namespace) -> Path:
    return journal.journal_path(args.run, args.journal_dir)


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# --- capture ---------------------------------------------------------------


def cmd_capture(args: argparse.Namespace) -> int:
    accounts = resolve_accounts(args)
    snapshot = state.capture(base_url(args), accounts, max_books=args.max_books)
    snapshot["captured_at"] = now_iso()
    snapshot["snapshot"] = args.snapshot
    out = Path(args.out) if args.out else journal_file(args).with_name("baseline.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(snapshot, indent=2) + "\n", encoding="utf-8")
    print(f"baseline for {len(accounts)} actor(s) over {len(snapshot.get('library') or [])} book(s) -> {out}")
    return 0


# --- check -----------------------------------------------------------------


def cmd_check(args: argparse.Namespace) -> int:
    path = journal_file(args)
    entries = journal.read_entries(path)
    accounts = resolve_accounts(args)
    url = base_url(args)

    baseline: dict[str, Any] | None = None
    baseline_path = Path(args.baseline) if args.baseline else path.with_name("baseline.json")
    if baseline_path.is_file():
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))

    findings: list[compare.Finding] = []
    unverifiable: list[expectations.Unverifiable] = []
    tallies: dict[str, int] = {}
    checked = 0
    seen_actors = journal.actors(entries)

    for actor in seen_actors:
        mine = journal.actor_entries(entries, actor)
        exps, unver, tally = expectations.expectations_for(actor, mine)
        unverifiable.extend(unver)
        for kind, count in tally.items():
            tallies[kind] = tallies.get(kind, 0) + count
        account = accounts.get(actor)
        if account is None:
            unverifiable.extend(
                expectations.Unverifiable(actor, e.seq, f"no credential for {actor} — state not readable")
                for e in exps
            )
            continue
        reader = state.open_actor(url, account)
        for exp in exps:
            checked += 1
            found = compare.check(exp, reader)
            if found is not None:
                findings.append(found)
        findings.extend(compare.unexpected(actor, reader, baseline, exps))

    report = {
        "run": args.run,
        "generated_at": now_iso(),
        "baseline_snapshot": (baseline or {}).get("snapshot"),
        "actors": seen_actors,
        "checked": checked,
        "findings": [f.to_json() for f in findings],
        "unverifiable": [{"actor": u.actor, "seq": u.seq, "why": u.why} for u in unverifiable],
    }
    out = Path(args.out) if args.out else path.with_name("audit.json")
    out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    writes = tallies.get(vocabulary.WRITE, 0)
    print(f"{path.name}: {len(entries)} entries, {checked} expectation(s) checked -> {out}")
    print("  classified: " + ", ".join(f"{k}={v}" for k, v in sorted(tallies.items())))
    # Say so out loud: an agent restates one value many times, and a reader who
    # only saw "38 writes / 19 checked" would reasonably suspect a silent drop.
    print(f"  {writes} write entr(y/ies) folded to {checked} expectation(s) "
          f"({len(unverifiable)} not judged)")
    if baseline is None:
        print("  no baseline — 'unexpected' findings are suppressed (run `audit.py capture` before a run)")
    for finding in findings:
        print(f"  [{finding.kind}] {finding.actor} seq={finding.seq} {finding.what} {finding.target}")
        print(f"      expected: {finding.expected}")
        print(f"      observed: {finding.observed}")
    if not findings:
        print("  no findings")
    print(f"  unverifiable: {len(unverifiable)}")
    return 1 if findings and args.strict else 0


# --- replay ----------------------------------------------------------------


def cmd_replay(args: argparse.Namespace) -> int:
    entries = journal.read_entries(journal_file(args))
    accounts = resolve_accounts(args)
    url = base_url(args)
    actors = [args.actor] if args.actor else journal.actors(entries)
    steps: list[replay.Step] = []

    for actor in actors:
        account = accounts.get(actor)
        if account is None:
            raise SystemExit(f"no credential for {actor} in --accounts")
        mine = [e for e in journal.actor_entries(entries, actor) if (e.seq or 0) >= args.start]
        exps, _, _ = expectations.expectations_for(actor, mine)
        if args.dry_run:
            reader = None
        else:
            reader = state.open_actor(url, account)
        for exp in exps:
            if reader is None:
                steps.append(replay.Step(actor, exp.seq, exp.family, exp.target, "would replay", None, exp.expected))
            else:
                steps.append(replay.replay_one(exp, reader))

    for step in steps:
        mark = "ok " if step.ok else ("-- " if step.status is None else "ERR")
        print(f"  {mark} {step.actor} seq={step.seq} {step.action} [{step.status}] {step.detail[:100]}")
    failed = [s for s in steps if s.status is not None and not s.ok]
    print(f"{len(steps)} step(s), {len(failed)} failed, {sum(1 for s in steps if s.status is None)} refused/dry")
    return 1 if failed else 0


# --- vocab -----------------------------------------------------------------


def cmd_vocab(args: argparse.Namespace) -> int:
    """Report how every action name in a journal was classified.

    The audit's residual is deliberately loud, so this is the maintenance
    loop: anything under `unknown` is a verb an agent invented that nobody has
    told the audit how to check.
    """
    paths = [journal_file(args)] if args.run else sorted(journal.journal_root(args.journal_dir).glob("*/journal.jsonl"))
    buckets: dict[str, dict[str, int]] = {}
    for path in paths:
        for entry in journal.iter_entries(path):
            cls = vocabulary.classify(entry.action)
            key = cls.kind if cls.kind != vocabulary.WRITE else f"write:{cls.family}"
            buckets.setdefault(key, {})
            name = entry.action or "<none>"
            buckets[key][name] = buckets[key].get(name, 0) + 1
    for key in sorted(buckets):
        total = sum(buckets[key].values())
        print(f"{key}  ({total})")
        for name, count in sorted(buckets[key].items(), key=lambda kv: (-kv[1], kv[0])):
            print(f"    {count:4d}  {name}")
    return 1 if vocabulary.UNKNOWN in buckets and args.strict else 0


def main() -> int:
    env.load()
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", help="instance base URL (default: $OMNIBUS_EXPLORE_URL)")
    ap.add_argument("--journal-dir", help="default: $OMNIBUS_EXPLORE_JOURNAL_DIR")
    ap.add_argument("--accounts", help="provision.sh JSON, or '-' for stdin")
    ap.add_argument("--out", help="output path (default: next to the journal)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    cap = sub.add_parser("capture", help="write the pre-run baseline")
    cap.add_argument("--run", required=True)
    cap.add_argument("--snapshot", help="the snapshot.sh name this baseline pairs with")
    cap.add_argument("--max-books", type=int, default=200)
    cap.set_defaults(func=cmd_capture)

    chk = sub.add_parser("check", help="reconcile the journal against live state")
    chk.add_argument("--run", required=True)
    chk.add_argument("--baseline", help="default: baseline.json next to the journal")
    chk.add_argument("--strict", action="store_true", help="exit non-zero when there are findings")
    chk.set_defaults(func=cmd_check)

    rep = sub.add_parser("replay", help="re-issue a journal suffix")
    rep.add_argument("--run", required=True)
    rep.add_argument("--actor", help="default: every actor in the journal")
    rep.add_argument("--from", dest="start", type=int, default=1, help="first seq to replay")
    rep.add_argument("--dry-run", action="store_true", help="list what would be replayed, send nothing")
    rep.set_defaults(func=cmd_replay)

    voc = sub.add_parser("vocab", help="how each action name was classified")
    voc.add_argument("--run", help="default: every journal in the directory")
    voc.add_argument("--strict", action="store_true", help="exit non-zero on any unknown action")
    voc.set_defaults(func=cmd_vocab)

    args = ap.parse_args()
    try:
        return args.func(args)
    except (journal.JournalError, ApiError) as exc:
        print(f"audit: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
