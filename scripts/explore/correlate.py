#!/usr/bin/env python3
"""Load a run's journal, audit and server log, and correlate them by time.

The journal is the run's only durable record and the server log is the only
independent one. Neither is useful alone: a 500 in the log names a path and no
agent, and a journal entry names an agent and no server behaviour. Joining them
on the clock is what makes a multi-agent run legible — it is how one agent's
error gets attached to another agent's write a few seconds earlier.

Two rules shape the join, both learned from run r-20260828-02:

- **Identity beats proximity.** A log line carrying a book uuid is matched
  against journal entries whose `target` is that uuid, even when a closer entry
  exists. At 16:11:57 the server warned about a backwards audio write on book
  3dcedaed; the nearest entry in time was a *different* agent's merge on a
  different book ten seconds later. Nearest-in-time would have blamed it.
- **Journalling lags the act.** Agents batch several entries onto one timestamp
  (three at 16:12:36 in that run), so a causing entry can be stamped *after*
  the log line it caused. Attribution therefore searches both directions and
  always reports the signed delta rather than implying causation.

Action names are matched on their **dotted prefix**, never in full. `start.md`
gives examples rather than an enum, and across two runs agents invented 50+
leaf names for a couple of dozen acts (`book.open` / `book.detail.open` /
`book.view`). The head noun is stable where the leaf is not. The three names
`start.md` does pin — `flow.start`, `flow.end`, `anomaly` — are matched exactly,
because those are contract, and a report that missed one would misreport the
run's shape rather than merely under-group a table.
"""

from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path

UUID_RE = re.compile(
    r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-"
    r"[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b"
)

# Anything the server emits at these levels, or with one of these statuses, is a
# finding. 304 is a hit, not a miss; 2xx is the happy path.
FINDING_LEVELS = {"WARN", "ERROR"}
BORING_STATUSES = {304}

# Ranked worst-first. An agent inventing a severity outside this table is not an
# error — it sorts last under its own name rather than being silently dropped.
SEVERITY_ORDER = ["critical", "high", "medium", "low", "info"]
UNRANKED = "unranked"

# Requests the browser makes on its own. Attributing these to an agent's action
# is noise dressed as a finding; nothing else is suppressed.
IGNORED_PATHS = {"/favicon.ico"}

# tower-http logs one line per response; a handler that also warned logs its own
# just before. Both describe one request, so they are folded into one finding.
RESPONSE_TARGETS = {"tower_http::trace::on_response", "tower_http::trace::on_failure"}
RESPONSE_MESSAGES = {"finished processing request", "response failed"}
# Already rendered in the headline — repeating them under Fields is padding.
HEADLINE_FIELDS = {"status", "latency", "classification"}

# tower-http's on_failure line carries no `status` field, only prose. A 500 is
# the most valuable thing in the log, so it must not render as "response failed"
# with no path attached.
CLASSIFICATION_STATUS = re.compile(r"Status code:\s*(\d{3})")


def parse_ts(value: str) -> datetime:
    """Parse an RFC 3339 timestamp, tolerating the trailing `Z`."""
    return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)


def fmt_delta(seconds: float) -> str:
    """Render a signed second-offset the way a reader reads it: +3.2s, -31.0s."""
    return f"{seconds:+.1f}s"


def fmt_duration(delta: timedelta) -> str:
    total = int(delta.total_seconds())
    h, rem = divmod(max(total, 0), 3600)
    m, s = divmod(rem, 60)
    if h:
        return f"{h}h {m}m"
    if m:
        return f"{m}m {s}s"
    return f"{s}s"


@dataclass(frozen=True)
class Entry:
    """One journal line, with the file line number that reproduces it."""

    line: int
    ts: datetime
    actor: str
    flow: str
    seq: int | None
    action: str
    target: str | None
    params: dict
    outcome: str
    note: str | None

    @property
    def family(self) -> str:
        """The action's head noun — see the module docstring on prefixes."""
        return self.action.split(".", 1)[0]

    @property
    def severity(self) -> str:
        raw = str(self.params.get("severity", "")).strip().lower()
        return raw or UNRANKED

    @property
    def citation(self) -> str:
        seq = "seq ?" if self.seq is None else f"seq {self.seq}"
        return f"L{self.line} · {self.actor} {seq}"


@dataclass(frozen=True)
class FlowSpan:
    """A flow's start and end for one actor, either end possibly missing."""

    actor: str
    flow: str
    start: Entry | None
    end: Entry | None

    @property
    def verdict(self) -> str:
        if self.end is None:
            return "unclosed"
        return str((self.end.params or {}).get("verdict") or "unstated")

    def covers(self, when: datetime) -> bool:
        if self.start is None:
            return False
        if when < self.start.ts:
            return False
        return self.end is None or when <= self.end.ts


@dataclass
class Run:
    run_id: str
    path: Path
    entries: list[Entry]
    malformed: list[int] = field(default_factory=list)
    foreign_runs: set[str] = field(default_factory=set)

    @property
    def actors(self) -> list[str]:
        return sorted({e.actor for e in self.entries})

    @property
    def started(self) -> datetime:
        return self.entries[0].ts

    @property
    def ended(self) -> datetime:
        return self.entries[-1].ts

    @property
    def duration(self) -> timedelta:
        return self.ended - self.started

    @property
    def anomalies(self) -> list[Entry]:
        return [e for e in self.entries if e.action == "anomaly"]

    @property
    def base_url(self) -> str | None:
        """The instance an agent said it was driving, from any flow.start."""
        for e in self.entries:
            url = (e.params or {}).get("base_url")
            if isinstance(url, str) and url:
                return url
        return None

    def spans(self) -> list[FlowSpan]:
        """Pair flow.start with flow.end per actor, in order.

        Both halves are optional on purpose: a run that aborts leaves flows open,
        and r-20260828-01 recorded two `flow.end` lines for flows that had no
        `flow.start` at all. Dropping either case would render an aborted run as
        one that simply did less.
        """
        open_by_actor: dict[str, list[Entry]] = {}
        spans: list[FlowSpan] = []
        for e in self.entries:
            if e.action == "flow.start":
                open_by_actor.setdefault(e.actor, []).append(e)
            elif e.action == "flow.end":
                pending = open_by_actor.get(e.actor, [])
                match = next((s for s in pending if s.flow == e.flow), None)
                if match is not None:
                    pending.remove(match)
                spans.append(FlowSpan(e.actor, e.flow, match, e))
        for actor, pending in open_by_actor.items():
            spans.extend(FlowSpan(actor, s.flow, s, None) for s in pending)
        return sorted(spans, key=lambda s: (s.start or s.end).ts)

    def line_of(self, actor: str, seq: int) -> int | None:
        """Journal line for an (actor, seq) pair — how the audit cites a write."""
        for e in self.entries:
            if e.actor == actor and e.seq == seq:
                return e.line
        return None


def load_journal(path: Path, run_id: str | None = None) -> Run:
    """Read journal.jsonl, keeping line numbers so every finding can cite one."""
    entries: list[Entry] = []
    malformed: list[int] = []
    foreign: set[str] = set()
    seen_run: str | None = None
    for lineno, raw in enumerate(path.read_text().splitlines(), start=1):
        if not raw.strip():
            continue
        try:
            row = json.loads(raw)
            ts = parse_ts(row["ts"])
        except (ValueError, KeyError, TypeError):
            malformed.append(lineno)
            continue
        row_run = row.get("run")
        # The journal is shared and append-only. A line from another run in this
        # file is a harness bug worth naming, not a line to silently include.
        if run_id and row_run and row_run != run_id:
            foreign.add(str(row_run))
            continue
        if seen_run is None and row_run:
            seen_run = str(row_run)
        params = row.get("params")
        entries.append(
            Entry(
                line=lineno,
                ts=ts,
                actor=str(row.get("actor") or "?"),
                flow=str(row.get("flow") or "?"),
                seq=row.get("seq") if isinstance(row.get("seq"), int) else None,
                action=str(row.get("action") or "?"),
                target=row.get("target") or None,
                params=params if isinstance(params, dict) else {},
                outcome=str(row.get("outcome") or "?"),
                note=row.get("note") or None,
            )
        )
    if not entries:
        raise ValueError(f"{path} holds no usable journal entries")
    entries.sort(key=lambda e: (e.ts, e.line))
    return Run(run_id or seen_run or path.parent.name, path, entries, malformed, foreign)


@dataclass
class Audit:
    """The `audit.json` contract #2202 writes next to the journal."""

    path: Path
    run: str | None
    checked: int
    findings: list[dict]
    unverifiable: list[dict]
    baseline_snapshot: str | None
    generated_at: str | None


def load_audit(path: Path) -> Audit:
    data = json.loads(path.read_text())
    if not isinstance(data, dict):
        raise ValueError(f"{path} is not a JSON object")
    return Audit(
        path=path,
        run=data.get("run"),
        checked=int(data.get("checked") or 0),
        findings=[f for f in (data.get("findings") or []) if isinstance(f, dict)],
        unverifiable=[u for u in (data.get("unverifiable") or []) if isinstance(u, dict)],
        baseline_snapshot=data.get("baseline_snapshot"),
        generated_at=data.get("generated_at"),
    )


@dataclass(frozen=True)
class LogFinding:
    ts: datetime
    level: str
    target: str
    message: str
    status: int | None
    method: str | None
    path: str | None
    uuids: tuple[str, ...]
    detail: str
    request_scoped: bool

    @property
    def is_panic(self) -> bool:
        return "panic" in self.message.lower()

    @property
    def key(self) -> tuple:
        """Group identical repeats — but never two lines about different books."""
        return (self.level, self.status, self.method, self.path, self.message, self.uuids)

    @property
    def headline(self) -> str:
        if self.status and self.method and self.path:
            return f"{self.status} {self.method} {self.path}"
        return self.message or self.target

    @property
    def subtitle(self) -> str:
        """The message, unless the headline already said it."""
        if self.message.lower() in RESPONSE_MESSAGES or self.message == self.headline:
            return ""
        return self.message


def parse_server_log(lines, window: tuple[datetime, datetime]) -> list[LogFinding]:
    """Pick the findings out of the JSON log sink, inside the run window.

    A finding is a WARN or ERROR, or any response that is neither 2xx nor 304 —
    the issue's bar. Lines are kept in file order, which is time order.
    """
    start, end = window
    out: list[LogFinding] = []
    for raw in lines:
        raw = raw.strip()
        if not raw:
            continue
        try:
            row = json.loads(raw)
            ts = parse_ts(row["timestamp"])
        except (ValueError, KeyError, TypeError):
            continue
        if not (start <= ts <= end):
            continue
        fields = row.get("fields") if isinstance(row.get("fields"), dict) else {}
        span = row.get("span") if isinstance(row.get("span"), dict) else {}
        status = fields.get("status")
        if not isinstance(status, int):
            m = CLASSIFICATION_STATUS.search(str(fields.get("classification") or ""))
            status = int(m.group(1)) if m else None
        level = str(row.get("level") or "").upper()
        interesting = level in FINDING_LEVELS or (
            status is not None and not (200 <= status < 300) and status not in BORING_STATUSES
        )
        if not interesting:
            continue
        path = span.get("path")
        if path in IGNORED_PATHS:
            continue
        detail = json.dumps(
            {k: v for k, v in fields.items() if k not in HEADLINE_FIELDS and k != "message"},
            sort_keys=True,
        )
        blob = f"{path or ''} {json.dumps(fields, sort_keys=True)}"
        out.append(
            LogFinding(
                ts=ts,
                level=level,
                target=str(row.get("target") or ""),
                message=str(fields.get("message") or fields.get("classification") or ""),
                status=status,
                method=span.get("method"),
                path=path,
                uuids=tuple(sorted({u.lower() for u in UUID_RE.findall(blob)})),
                detail="" if detail == "{}" else detail,
                request_scoped=bool(span),
            )
        )
    return out


def fold_response_lines(findings: list[LogFinding], gap_s: float = 1.0) -> list[LogFinding]:
    """Merge a handler's own WARN/ERROR with tower-http's line for that response.

    They are one request seen twice — the covers handler warns "no cover image on
    record (404)" 41µs before tower-http logs the 404. Listing both doubles the
    findings without adding a fact, and a report that pads is a report nobody
    finishes.
    """
    out: list[LogFinding] = []
    taken: set[int] = set()
    for i, f in enumerate(findings):
        if i in taken or f.target in RESPONSE_TARGETS or not f.request_scoped:
            continue
        for j, g in enumerate(findings):
            if j in taken or j == i or g.target not in RESPONSE_TARGETS:
                continue
            if (g.method, g.path) != (f.method, f.path):
                continue
            if abs((g.ts - f.ts).total_seconds()) > gap_s:
                continue
            taken.add(j)
            f = replace(f, status=f.status if f.status is not None else g.status)
            break
        taken.add(i)
        out.append(f)
    out.extend(f for i, f in enumerate(findings) if i not in taken)
    return sorted(out, key=lambda f: f.ts)


def read_server_log(paths: list[Path], window) -> list[LogFinding]:
    lines: list[str] = []
    for p in paths:
        lines.extend(p.read_text(errors="replace").splitlines())
    return parse_server_log(lines, window)


# Cheap remote prefilter. The sink is mostly INFO request lines, so shipping the
# whole day back over ssh to throw 98% of it away is the wrong trade; the exact
# rules are still applied locally by parse_server_log.
PREFILTER = r'"level":"(WARN|ERROR)"|"status": *[3-5][0-9][0-9]|panic'


def fetch_server_log(
    window: tuple[datetime, datetime],
    host: str,
    remote_dir: str,
    log_dir: str,
    timeout: int = 120,
) -> tuple[list[LogFinding], str]:
    """Pull the run window's log findings off the instance over ssh.

    Returns the findings and a status line. A failure here must never read as a
    clean run, so the status is carried into the report rather than swallowed.
    """
    start, end = window
    days, cursor = [], start.date()
    while cursor <= end.date():
        days.append(cursor.isoformat())
        cursor += timedelta(days=1)
    files = " ".join(f"~/{remote_dir}/{log_dir}/omnibus.log.{d}" for d in days)
    cmd = f"grep -hE '{PREFILTER}' {files} 2>/dev/null || true"
    try:
        proc = subprocess.run(
            ["ssh", "-o", "ConnectTimeout=15", "-o", "BatchMode=yes", host, cmd],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return [], f"unavailable — ssh to {host} failed: {exc}"
    if proc.returncode != 0:
        err = (proc.stderr or "").strip().splitlines()
        return [], f"unavailable — ssh to {host} exited {proc.returncode}: {err[-1] if err else ''}"
    findings = parse_server_log(proc.stdout.splitlines(), window)
    return findings, f"read from {host}:~/{remote_dir}/{log_dir} for {', '.join(days)}"


@dataclass
class Attribution:
    """Which agent action a server-log finding most plausibly belongs to."""

    entry: Entry | None
    basis: str
    delta: float
    in_flight: list[tuple[str, str]]
    neighbours: list[tuple[Entry, float]]

    @property
    def cite(self) -> str:
        if self.entry is None:
            return "no journal entry in window"
        return f"{self.entry.citation} · {self.entry.action} ({fmt_delta(self.delta)}, by {self.basis})"


def attribute(
    finding: LogFinding, run: Run, spans: list[FlowSpan], window_s: float = 90.0
) -> Attribution:
    """Attach one log finding to the agent action that most likely caused it.

    Identity first: if the line names a book uuid, only entries whose `target`
    is that uuid are candidates. Time second, and in both directions — see the
    module docstring on batched journalling. Everything else the reader needs to
    judge the join is returned alongside: what each agent had in flight at that
    instant, and the other actors' entries inside the window.
    """
    in_window = [e for e in run.entries if abs((e.ts - finding.ts).total_seconds()) <= window_s]
    by_target = [e for e in in_window if e.target and e.target.lower() in finding.uuids]

    candidates, basis = (by_target, "target uuid") if by_target else (in_window, "time")
    # Later entries win a tie: an agent stamps the journal after the act, so the
    # entry that follows a log line is the likelier cause of it.
    best = min(candidates, key=lambda e: (abs((e.ts - finding.ts).total_seconds()), -e.ts.timestamp()), default=None)

    in_flight = sorted({(s.actor, s.flow) for s in spans if s.covers(finding.ts)})
    # Only *other* actors: what makes a multi-agent run legible is seeing whose
    # work sat next to whose, and the attributed agent's own entries are already
    # in the timeline under their flow.
    neighbours = [
        (e, (e.ts - finding.ts).total_seconds())
        for e in in_window
        if e is not best and (best is None or e.actor != best.actor) and e.action != "anomaly"
    ]
    neighbours.sort(key=lambda pair: abs(pair[1]))
    return Attribution(
        entry=best,
        basis="none" if best is None else basis,
        delta=0.0 if best is None else (best.ts - finding.ts).total_seconds(),
        in_flight=in_flight,
        neighbours=neighbours[:3],
    )


@dataclass
class LogGroup:
    """Identical repeats of one finding, attributed occurrence by occurrence."""

    findings: list[LogFinding]
    attributions: list[Attribution]

    @property
    def first(self) -> LogFinding:
        return self.findings[0]

    @property
    def count(self) -> int:
        return len(self.findings)

    @property
    def primary(self) -> Attribution:
        return self.attributions[0]

    @property
    def actors(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for a in self.attributions:
            name = a.entry.actor if a.entry else "unattributed"
            out[name] = out.get(name, 0) + 1
        return dict(sorted(out.items(), key=lambda kv: (-kv[1], kv[0])))


def group_and_attribute(
    findings: list[LogFinding], run: Run, window_s: float = 90.0
) -> list[LogGroup]:
    """Collapse identical repeats, attributing every occurrence, worst first."""
    spans = run.spans()
    buckets: dict[tuple, list[LogFinding]] = {}
    for f in fold_response_lines(findings):
        buckets.setdefault(f.key, []).append(f)
    groups = [
        LogGroup(fs, [attribute(f, run, spans, window_s) for f in fs])
        for fs in buckets.values()
    ]

    def rank(g: LogGroup) -> tuple:
        f = g.first
        return (
            0 if f.is_panic else 1,
            0 if f.level == "ERROR" else 1,
            0 if (f.status or 0) >= 500 else 1,
            0 if f.request_scoped else 1,
            f.ts,
        )

    return sorted(groups, key=rank)


def severity_rank(severity: str) -> tuple[int, str]:
    """Sort key for an anomaly severity, unknown names last but not dropped."""
    try:
        return (SEVERITY_ORDER.index(severity), severity)
    except ValueError:
        return (len(SEVERITY_ORDER), severity)


def ranked_anomalies(run: Run) -> list[Entry]:
    """Anomalies worst-first, then oldest-first inside a severity."""
    return sorted(run.anomalies, key=lambda e: (severity_rank(e.severity), e.ts, e.line))
