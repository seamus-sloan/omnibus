#!/usr/bin/env python3
"""Draw the flow sequence for an exploration run.

The agent never samples — see "you never sample anything yourself" in
docs/qa/agentic_exploration/start.md. An LLM told "pick a flow at random" will
not produce a uniform draw; it will produce the same three flows every time.
So the runner owns the dice and hands over one flow at a time.

Every top-level flow is equally likely. There is no weight column: the run
is looking for defects, not modelling how often a reader does something, and
a weighted draw over a distinct sample mostly decided which low-weight flows
never ran at all. A subflow always runs inside its parent, for the same
reason: a roll that skips it is a roll that skips a check.

The catalog table in flows/README.md is the single source of truth for which
flows exist and which parent a subflow runs inside. The parser is
deliberately strict: if the table cannot be read, or a subflow names a parent
that is not a top-level flow, this exits non-zero rather than silently
drawing from a catalog nobody intended.
"""

from __future__ import annotations

import argparse
import json
import random
import re
import sys
from pathlib import Path

ROW = re.compile(r"^\|\s*\[([a-z_]+)\]\([^)]+\)\s*\|\s*([^|]+?)\s*\|")
TOP = "on its own"
INSIDE = re.compile(r"^inside\s+([a-z_]+)$")


def parse_catalog(path: Path) -> tuple[list[str], dict[str, list[str]]]:
    top: list[str] = []
    subs: dict[str, list[str]] = {}
    for line in path.read_text().splitlines():
        m = ROW.match(line)
        if not m:
            continue
        name, runs = m.group(1), m.group(2).strip()
        if runs == TOP:
            top.append(name)
        elif inside := INSIDE.match(runs):
            subs.setdefault(inside.group(1), []).append(name)
        else:
            sys.exit(f"unparseable 'Runs' cell in catalog for {name}: {runs!r}")

    if not top:
        sys.exit(f"no top-level flows parsed from {path} — has the table format changed?")
    for parent in subs:
        if parent not in top:
            sys.exit(f"subflow parent {parent!r} is not a top-level flow — fix {path}")
    return top, subs


def draw(top, subs, count, rng, first=None):
    """Draw `count` distinct flows, uniformly. `first` is forced to the front."""
    pool = list(top)
    picked: list[str] = []
    if first:
        if first not in pool:
            sys.exit(f"--first {first} is not a top-level flow")
        picked.append(first)
        pool.remove(first)
    picked.extend(rng.sample(pool, min(count - len(picked), len(pool))))

    return [
        {"flow": name, "subflows": list(subs.get(name, []))}
        for name in picked
    ]


def main() -> None:
    ap = argparse.ArgumentParser()
    # Matches the default the skill's argument table shows the user, so the
    # two cannot drift into disagreeing about what a bare run means.
    ap.add_argument("--agents", type=int, default=2)
    ap.add_argument("--flows-per-agent", type=int, default=4)
    ap.add_argument("--seed", type=int, default=None,
                    help="omit to draw one; the seed used is always emitted, "
                         "and re-running with it reproduces the draw exactly")
    ap.add_argument("--run", required=True)
    ap.add_argument("--catalog", type=Path,
                    default=Path(__file__).resolve().parents[2]
                    / "docs/qa/agentic_exploration/flows/README.md")
    ap.add_argument("--library-empty", action="store_true",
                    help="force adding_book first for every agent: with no books, "
                         "most flows have nothing to act on")
    ap.add_argument("--exclude", default="",
                    help="comma-separated flows to drop (e.g. a flow whose "
                         "content the corpus cannot supply)")
    args = ap.parse_args()

    if args.agents < 1:
        sys.exit("--agents must be at least 1")
    if args.flows_per_agent < 1:
        sys.exit("--flows-per-agent must be at least 1")

    top, subs = parse_catalog(args.catalog)
    excluded = {s.strip() for s in args.exclude.split(",") if s.strip()}
    top = [t for t in top if t not in excluded]
    subs = {p: [s for s in names if s not in excluded] for p, names in subs.items() if p not in excluded}

    # Flows are drawn distinct, so asking for more than exist would silently
    # yield a shorter sequence — a coverage cut that reads as coverage.
    if args.flows_per_agent > len(top):
        sys.exit(f"--flows-per-agent {args.flows_per_agent} exceeds the "
                 f"{len(top)} flow(s) available after exclusions")

    # A run with no seed is still reproducible, because the seed is emitted.
    seed = args.seed if args.seed is not None else random.SystemRandom().randrange(2**31)
    rng = random.Random(seed)
    agents = [
        {
            "actor": f"agent-{i}",
            "sequence": draw(top, subs, args.flows_per_agent, rng,
                             first="adding_book" if args.library_empty else None),
        }
        for i in range(1, args.agents + 1)
    ]
    json.dump({"run": args.run, "seed": seed, "agents": agents}, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
