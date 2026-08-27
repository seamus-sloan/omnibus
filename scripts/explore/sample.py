#!/usr/bin/env python3
"""Draw the flow sequence for an exploration run.

The agent never samples — see "you never sample anything yourself" in
docs/qa/agentic_exploration/start.md. An LLM told "20% chance to browse authors"
will not produce that distribution; it will produce roughly never or roughly
always. So the runner owns the dice and hands over one flow at a time.

Weights come from the catalog table in flows/README.md, which is therefore the
single source of truth rather than documentation of a config kept elsewhere. The
parser is deliberately strict: if the table cannot be read, or the top-level
weights do not sum to 100, this exits non-zero rather than silently sampling
from a distribution nobody intended.
"""

from __future__ import annotations

import argparse
import json
import random
import re
import sys
from pathlib import Path

ROW = re.compile(r"^\|\s*\[([a-z_]+)\]\([^)]+\)\s*\|\s*([^|]+?)\s*\|")
BARE = re.compile(r"^(\d+)%$")
COND = re.compile(r"^(\d+)%\s+of\s+an?\s+(.+?)\s+flow$")

# Which parent each conditional weight attaches to. The table phrases these in
# prose ("50% of a reading flow"), so map that phrasing onto the flow name.
PARENT = {
    "reading": "reading_a_book",
    "listening": "listening_to_audiobook",
    "details": "browsing_book_details",
    "add-a-book": "adding_book",
}


def parse_catalog(path: Path) -> tuple[dict[str, int], dict[str, list[tuple[str, int]]]]:
    top: dict[str, int] = {}
    subs: dict[str, list[tuple[str, int]]] = {}
    for line in path.read_text().splitlines():
        m = ROW.match(line)
        if not m:
            continue
        name, weight = m.group(1), m.group(2).strip()
        if bare := BARE.match(weight):
            top[name] = int(bare.group(1))
        elif cond := COND.match(weight):
            parent = PARENT.get(cond.group(2))
            if parent is None:
                sys.exit(f"unknown parent phrasing in catalog: {weight!r}")
            subs.setdefault(parent, []).append((name, int(cond.group(1))))
        else:
            sys.exit(f"unparseable weight in catalog for {name}: {weight!r}")

    if not top:
        sys.exit(f"no top-level flows parsed from {path} — has the table format changed?")
    total = sum(top.values())
    if total != 100:
        sys.exit(f"top-level weights sum to {total}, not 100 — fix {path}")
    return top, subs


def draw(top, subs, count, rng, first=None):
    """Draw `count` distinct flows, weighted. `first` is forced to the front."""
    pool = dict(top)
    picked: list[str] = []
    if first:
        if first not in pool:
            sys.exit(f"--first {first} is not a top-level flow")
        picked.append(first)
        pool.pop(first)
    while len(picked) < count and pool:
        name = rng.choices(list(pool), weights=list(pool.values()))[0]
        picked.append(name)
        pool.pop(name)

    out = []
    for name in picked:
        rolled = [s for s, w in subs.get(name, []) if rng.random() < w / 100]
        out.append({"flow": name, "subflows": rolled})
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--agents", type=int, default=1)
    ap.add_argument("--flows-per-agent", type=int, default=4)
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("--run", required=True)
    ap.add_argument("--catalog", type=Path,
                    default=Path(__file__).resolve().parents[2]
                    / "docs/qa/agentic_exploration/flows/README.md")
    ap.add_argument("--library-empty", action="store_true",
                    help="force adding_book first for every agent: with no books, "
                         "ten of the flows have nothing to act on")
    ap.add_argument("--exclude", default="",
                    help="comma-separated flows to drop (e.g. a flow whose "
                         "content the corpus cannot supply)")
    args = ap.parse_args()

    top, subs = parse_catalog(args.catalog)
    for name in filter(None, (s.strip() for s in args.exclude.split(","))):
        top.pop(name, None)

    rng = random.Random(args.seed)
    agents = [
        {
            "actor": f"agent-{i}",
            "sequence": draw(top, subs, args.flows_per_agent, rng,
                             first="adding_book" if args.library_empty else None),
        }
        for i in range(1, args.agents + 1)
    ]
    json.dump({"run": args.run, "seed": args.seed, "agents": agents}, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
