"""Read and append the shared run journal.

One JSONL file per run, appended concurrently by every agent. Two properties
matter and neither is free:

* **A line is never interleaved or truncated.** Appends take an exclusive
  `flock` and issue the whole record — newline included — as a single
  `os.write` to a descriptor opened `O_APPEND`. `print(..., file=f)` does not
  qualify: Python's buffered writer is free to split a record across syscalls,
  and two agents flushing halves is exactly the corruption `owned.sh` refuses
  to parse past.
* **`seq` is monotonic and unique per actor.** It is derived under the same
  lock from the journal filtered to that actor, so two of an agent's own
  tools cannot mint the same number. Counting all lines instead would number
  an agent by other agents' work.
"""

from __future__ import annotations

import errno
import fcntl
import json
import os
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterator

RUN_DIR = re.compile(r"^r-[0-9A-Za-z._-]+$")

# The field order start.md documents. Cosmetic, but a journal is read by eye
# as often as by script.
FIELD_ORDER = ("ts", "run", "actor", "surface", "flow", "seq", "action", "target", "params", "outcome", "note")


class JournalError(Exception):
    """A journal could not be read or appended to."""


@dataclass(frozen=True)
class Entry:
    """One journal line, with the raw object kept for anything unmodelled."""

    ts: str | None
    run: str | None
    actor: str | None
    surface: str | None
    flow: str | None
    seq: int | None
    action: str | None
    target: str | None
    outcome: str | None
    note: str | None
    params: dict[str, Any] = field(default_factory=dict)
    raw: dict[str, Any] = field(default_factory=dict)
    # Why a null `target` could not be filled from the title the entry names —
    # set by `expectations.resolve_targets`, never read from the file.
    target_note: str | None = None

    @classmethod
    def from_obj(cls, obj: dict[str, Any]) -> "Entry":
        params = obj.get("params")
        seq = obj.get("seq")
        return cls(
            ts=obj.get("ts"),
            run=obj.get("run"),
            actor=obj.get("actor"),
            surface=obj.get("surface"),
            flow=obj.get("flow"),
            seq=seq if isinstance(seq, int) else None,
            action=obj.get("action"),
            target=obj.get("target"),
            outcome=obj.get("outcome"),
            note=obj.get("note"),
            params=params if isinstance(params, dict) else {},
            raw=obj,
        )


def journal_root(explicit: str | os.PathLike[str] | None = None) -> Path:
    """Resolve the directory holding `<run-id>/journal.jsonl`.

    Order: an explicit path, then `OMNIBUS_EXPLORE_JOURNAL_DIR`. The journal
    is the ownership ledger, so it must live outside the worktrees — see the
    same reasoning in `owned.sh`. There is no in-repo default here on purpose:
    silently writing to a per-worktree path is how a `wt switch` orphans every
    book a previous run uploaded.
    """
    raw = explicit or os.environ.get("OMNIBUS_EXPLORE_JOURNAL_DIR")
    if not raw:
        raise JournalError(
            "no journal directory: pass --journal-dir or set "
            "OMNIBUS_EXPLORE_JOURNAL_DIR (see .env.example)"
        )
    return Path(os.path.expandvars(os.path.expanduser(str(raw)))).resolve()


def journal_path(run: str, root: str | os.PathLike[str] | None = None) -> Path:
    """Path to one run's journal file."""
    if not RUN_DIR.match(run):
        raise JournalError(f"implausible run id {run!r} — expected r-YYYYMMDD-NN")
    return journal_root(root) / run / "journal.jsonl"


def iter_entries(path: str | os.PathLike[str]) -> Iterator[Entry]:
    """Stream a journal, failing loudly on a torn line.

    A journal that cannot be fully parsed must not yield a shorter list: a
    dropped line is a write the audit would then never look for, which is the
    silent pass this whole system exists to prevent.
    """
    p = Path(path)
    try:
        handle = p.open(encoding="utf-8")
    except OSError as exc:
        raise JournalError(f"cannot read journal {p}: {exc}") from exc
    with handle:
        for lineno, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError as exc:
                raise JournalError(f"{p}:{lineno}: unparseable journal line ({exc}): {line[:120]}") from exc
            if not isinstance(obj, dict):
                raise JournalError(f"{p}:{lineno}: journal line is not an object: {line[:120]}")
            yield Entry.from_obj(obj)


def read_entries(path: str | os.PathLike[str]) -> list[Entry]:
    """Read a whole journal into memory, ordered as written."""
    return list(iter_entries(path))


def actor_entries(entries: list[Entry], actor: str) -> list[Entry]:
    """One actor's entries in `seq` order, with unsequenced ones kept last.

    Ordering is by `seq` rather than by `ts` because `seq` is the actor's own
    monotonic counter; timestamps come from three machines' clocks and only
    have to be good enough to correlate agents against each other.
    """
    mine = [e for e in entries if e.actor == actor]
    return sorted(mine, key=lambda e: (e.seq is None, e.seq or 0))


def actors(entries: list[Entry]) -> list[str]:
    """Every actor that appears in the journal, in first-seen order."""
    seen: dict[str, None] = {}
    for e in entries:
        if e.actor:
            seen.setdefault(e.actor, None)
    return list(seen)


def _open_locked(path: Path):
    """Open `path` append-only and take an exclusive lock on it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
    except OSError as exc:
        raise JournalError(f"cannot open journal {path}: {exc}") from exc
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
    except OSError as exc:  # pragma: no cover - only on exotic filesystems
        os.close(fd)
        if exc.errno in (errno.ENOLCK, errno.EOPNOTSUPP):
            raise JournalError(
                f"{path} is on a filesystem without locking — put the journal "
                "on local disk, not a network share"
            ) from exc
        raise JournalError(f"cannot lock journal {path}: {exc}") from exc
    return fd


def next_seq(path: str | os.PathLike[str], actor: str) -> int:
    """The next free `seq` for `actor`, one past their highest so far.

    Scans the journal backwards and stops at the actor's most recent entry:
    per-actor seqs are monotonic (minted under the append lock), so the last
    entry an actor wrote carries their highest seq. An actor's own last write
    is almost always near the tail, making the common append O(tail) rather
    than O(file) — without a sidecar index, which would be a second copy of a
    fact the journal already holds and could desync from it.
    """
    p = Path(path)
    if not p.exists():
        return 1
    for raw in _lines_reversed(p):
        raw = raw.strip()
        if not raw:
            continue
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if obj.get("actor") == actor and isinstance(obj.get("seq"), int):
            return obj["seq"] + 1
    return 1


def _lines_reversed(p: Path, block: int = 65536):
    """Yield the file's lines last-first without reading it whole."""
    with p.open("rb") as fh:
        fh.seek(0, os.SEEK_END)
        pos = fh.tell()
        tail = b""
        while pos > 0:
            step = min(block, pos)
            pos -= step
            fh.seek(pos)
            chunk = fh.read(step) + tail
            lines = chunk.split(b"\n")
            tail = lines.pop(0)
            for line in reversed(lines):
                yield line.decode("utf-8", errors="replace")
        if tail:
            yield tail.decode("utf-8", errors="replace")


def append(path: str | os.PathLike[str], record: dict[str, Any]) -> dict[str, Any]:
    """Append one record atomically, filling in `ts` and `seq` if absent.

    Returns the record as written. The lock is held across the seq lookup and
    the write, so two concurrent appends by the same actor cannot collide on a
    number.
    """
    if not isinstance(record, dict):
        raise JournalError("journal record must be a JSON object")
    p = Path(path)
    fd = _open_locked(p)
    try:
        out = dict(record)
        out.setdefault("ts", datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z")
        if out.get("seq") is None:
            actor = out.get("actor")
            if not actor:
                raise JournalError("journal record needs an actor to derive seq")
            out["seq"] = next_seq(p, str(actor))
        ordered = {k: out[k] for k in FIELD_ORDER if k in out}
        ordered.update({k: v for k, v in out.items() if k not in ordered})
        out = ordered
        line = (json.dumps(out, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
        if b"\n" in line[:-1]:  # pragma: no cover - json.dumps escapes newlines
            raise JournalError("record serialised to more than one line")
        written = 0
        while written < len(line):
            written += os.write(fd, line[written:])
        os.fsync(fd)
        return out
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)
