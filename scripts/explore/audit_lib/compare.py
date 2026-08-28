"""Compare folded expectations against live state and emit findings.

A finding is one of four kinds, and which one is chosen matters as much as
whether one is raised at all:

* `missing` — journalled, absent from state. The write was lost.
* `mismatch` — present, holding something else.
* `unexpected` — present now, absent from the baseline, and nothing in the
  journal claims it. Only ever raised with a baseline, because without one
  every fact two earlier runs left behind looks unexplained.
* `duplicate` — landed more than once.

The bias throughout is against false positives. An expectation the audit
could not read never becomes a finding (it is `unverifiable` instead) — and
its slot still counts as journalled: `unexpected` subtracts `Claims`, the
slots every entry addressed whether or not it was judged, because a write the
audit declined to read usually still landed. An
absent read-status row is read as `unread` because that is what the column
means, and a saved position is checked for existence on the right axis rather
than for an exact number — the reader keeps writing after the journal line is
appended, so any exact positional comparison would fail on a healthy run.

Nothing here compares a **derived** player value — elapsed, remaining, or any
rate-adjusted clock. Only the stored position and the stored playback rate are
read. #2246 makes the player clocks rate-adjusted, so a mid-book 1× → 2×
switch halves elapsed *on purpose*; an audit that had learned today's clock
arithmetic would start reporting that intended behaviour as a defect.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any

from .client import ApiError
from .expectations import Claims, Expectation, normalise_text
from .state import ActorState

MISSING = "missing"
MISMATCH = "mismatch"
UNEXPECTED = "unexpected"
DUPLICATE = "duplicate"

RATE_TOLERANCE = 0.01


@dataclass
class Finding:
    actor: str
    seq: int | None
    kind: str
    what: str
    target: str | None
    expected: str
    observed: str
    replay_from: int | None

    def to_json(self) -> dict[str, Any]:
        return asdict(self)


def _finding(exp: Expectation, kind: str, observed: str) -> Finding:
    return Finding(exp.actor, exp.seq, kind, exp.what, exp.target, exp.expected, observed, exp.seq)


def _describe_rating(stars: float | None) -> str:
    return "no rating" if stars is None else f"rating {float(stars):g} of 5"


def check(exp: Expectation, state: ActorState) -> Finding | None:
    """Check one expectation against live state. `None` means it held."""
    handler = _HANDLERS.get(exp.family)
    if handler is None:  # pragma: no cover - guarded by the vocabulary table
        return _finding(exp, MISSING, f"audit has no check for family {exp.family!r}")
    return handler(exp, state)


def check_all(
    exps: list[Expectation], state: ActorState
) -> tuple[list[Finding], list[tuple[Expectation, str]]]:
    """Check every expectation, containing a failed read to that expectation.

    One agent journalling a slug where a uuid belongs makes the state read
    error (the RPC 500s on an unresolvable uuid, and `Client.rpc` raises).
    That must cost the run one unverifiable entry, not the whole `audit.json`.
    """
    findings: list[Finding] = []
    failed: list[tuple[Expectation, str]] = []
    for exp in exps:
        try:
            finding = check(exp, state)
        except ApiError as exc:
            failed.append((exp, f"{exp.what} on {exp.target}: state read failed — {exc}"))
            continue
        if finding is not None:
            findings.append(finding)
    return findings, failed


def _check_rating(exp: Expectation, state: ActorState) -> Finding | None:
    observed = state.rating(exp.target or "")
    if exp.value is None:
        return None if observed is None else _finding(exp, MISMATCH, _describe_rating(observed))
    if observed is None:
        return _finding(exp, MISSING, "no rating on the book")
    if abs(float(observed) - float(exp.value)) > 1e-6:
        return _finding(exp, MISMATCH, _describe_rating(observed))
    return None


def _check_read_status(exp: Expectation, state: ActorState) -> Finding | None:
    observed = state.read_status(exp.target or "")
    # A missing row *is* `unread` (shared::ReadStatus::Unread documents it), so
    # an agent that journalled a return to unread has not lost a write.
    effective = observed or "unread"
    if effective == exp.value:
        return None
    kind = MISSING if observed is None else MISMATCH
    return _finding(exp, kind, "no read-status row" if observed is None else f"read status {observed!r}")


def _check_progress(exp: Expectation, state: ActorState) -> Finding | None:
    # Existence on the claimed axis, never the number: the position keeps
    # moving after the entry is written, and elapsed/remaining are derived
    # values whose arithmetic is deliberately changing (#2246).
    axis = (exp.value or {}).get("axis") or "ebook"
    record = state.progress(exp.target or "", axis)
    if not record:
        return _finding(exp, MISSING, f"no saved position for this book on the {axis} axis")
    if axis == "audio":
        if record.get("audio_position_seconds") is None:
            return _finding(exp, MISMATCH, f"a position exists but carries no audio offset: {_thin(record)}")
    elif record.get("epub_cfi") is None and record.get("progress_percent") is None:
        return _finding(exp, MISMATCH, f"a position exists but carries no reading location: {_thin(record)}")
    return None


def _check_playback_rate(exp: Expectation, state: ActorState) -> Finding | None:
    observed = state.playback_rate(exp.target or "")
    if observed is None:
        return _finding(exp, MISSING, "no saved playback rate")
    if abs(float(observed) - float(exp.value)) > RATE_TOLERANCE:
        return _finding(exp, MISMATCH, f"playback rate {float(observed):g}x")
    return None


def _check_journal(exp: Expectation, state: ActorState) -> Finding | None:
    wanted = normalise_text(str(exp.value))
    rows = state.journals(exp.target or "")
    hits = [r for r in rows if wanted in normalise_text(r.get("body_md") or "")]
    if len(hits) > 1:
        return _finding(exp, DUPLICATE, f"{len(hits)} journal entries carry this text")
    if hits:
        return None
    phrase = exp.extra.get("phrase")
    if phrase and any(normalise_text(phrase) in normalise_text(r.get("body_md") or "") for r in rows):
        return _finding(exp, MISMATCH, "an entry with the distinctive phrase exists but its text differs")
    if not rows:
        return _finding(exp, MISSING, "no journal entry by this actor on the book")
    return _finding(exp, MISMATCH, f"{len(rows)} journal entr(y/ies) on the book, none carrying this text")


# The identifying keys an annotation expectation may carry, and the row field
# each one is compared against.
_ANNOTATION_FIELDS = (("note", "note"), ("quote", "text"), ("label", "title"))


def _annotation_matches(row: dict[str, Any], keys: dict[str, Any]) -> bool:
    for key, row_field in _ANNOTATION_FIELDS:
        wanted = keys.get(key)
        if wanted and normalise_text(str(wanted)) not in normalise_text(str(row.get(row_field) or "")):
            return False
    colour = keys.get("colour")
    if colour and str(colour).strip().lower() != str(row.get("color") or "").strip().lower():
        return False
    # A position is comparable only when it is a machine location — prose
    # ("Prologue, first page") never equals the stored CFI, and comparing it
    # would fail every honestly-journalled bookmark.
    position = keys.get("position")
    if position and "epubcfi(" in str(position):
        if normalise_text(str(position)) != normalise_text(str(row.get("position") or "")):
            return False
    return True


def _check_annotation(exp: Expectation, state: ActorState, rows: list[dict[str, Any]], noun: str) -> Finding | None:
    keys = exp.value if isinstance(exp.value, dict) else {}
    hits = [r for r in rows if _annotation_matches(r, keys)]
    identified = any(keys.get(k) for k in ("note", "quote", "label"))
    if len(hits) > 1 and identified:
        return _finding(exp, DUPLICATE, f"{len(hits)} {noun}s match")
    if hits:
        return None
    if not rows:
        return _finding(exp, MISSING, f"no {noun}s on the book")
    return _finding(exp, MISMATCH, f"{len(rows)} {noun}(s) on the book, none matching")


def _check_highlight(exp: Expectation, state: ActorState) -> Finding | None:
    return _check_annotation(exp, state, state.highlights(exp.target or ""), "highlight")


def _check_bookmark(exp: Expectation, state: ActorState) -> Finding | None:
    return _check_annotation(exp, state, state.bookmarks(exp.target or ""), "bookmark")


def _check_shelf(exp: Expectation, state: ActorState) -> Finding | None:
    names = [s.get("name") for s in state.shelves()]
    hits = [n for n in names if n == exp.value]
    if len(hits) > 1:
        return _finding(exp, DUPLICATE, f"{len(hits)} shelves named {exp.value!r}")
    if hits:
        return None
    return _finding(exp, MISSING, f"this actor owns {len(names)} shelf/shelves, none named {exp.value!r}")


def _check_shelf_member(exp: Expectation, state: ActorState) -> Finding | None:
    shelf = next((s for s in state.shelves() if s.get("name") == exp.target), None)
    if shelf is None:
        return _finding(exp, MISSING, f"no shelf named {exp.target!r} to hold it")
    members = state.shelf_members(int(shelf["id"]))
    if exp.value in members:
        return None
    return _finding(exp, MISSING, f"shelf holds {len(members)} book(s), not this one")


def _check_wishlist(exp: Expectation, state: ActorState) -> Finding | None:
    entries = state.wishlist()
    if exp.value in entries:
        return None
    return _finding(exp, MISSING, f"wishlist holds {len(entries)} entr(y/ies), not this one")


def _check_book_add(exp: Expectation, state: ActorState) -> Finding | None:
    if exp.value in state.library():
        return None
    return _finding(exp, MISSING, "book is not in the library listing")


_HANDLERS = {
    "rating": _check_rating,
    "read_status": _check_read_status,
    "progress": _check_progress,
    "playback_rate": _check_playback_rate,
    "journal": _check_journal,
    "highlight": _check_highlight,
    "bookmark": _check_bookmark,
    "shelf": _check_shelf,
    "shelf_member": _check_shelf_member,
    "wishlist": _check_wishlist,
    "book_add": _check_book_add,
}


def _thin(record: dict[str, Any]) -> str:
    keep = ("format", "epub_cfi", "audio_position_seconds", "progress_percent")
    return ", ".join(f"{k}={record.get(k)!r}" for k in keep)


def unexpected(
    actor: str,
    state: ActorState,
    baseline: dict[str, Any] | None,
    claims: Claims,
) -> list[Finding]:
    """State that appeared during the run and no journal entry explains.

    Subtracts `Claims` — every slot the journal *addressed*, judged or not —
    rather than only the checked expectations: an unverifiable write usually
    still landed, and re-reporting it here would contradict the journal line
    that names it. The claims also carry the read-status slots of every book
    an agent merely opened, because the reading surfaces auto-write status
    with nothing journalled (frontend/src/read_status_auto.rs).

    Requires a baseline. Without one there is nothing to subtract, and every
    fact left behind by an earlier run would be reported as a surprise.
    """
    if not baseline:
        return []
    before = (baseline.get("actors") or {}).get(actor) or {}
    books = before.get("books") or {}
    out: list[Finding] = []

    for uuid in baseline.get("library") or []:
        was = books.get(uuid) or {}
        if ("rating", uuid) not in claims.slots:
            now = state.rating(uuid)
            if now is not None and was.get("rating") != now:
                out.append(_bare(actor, UNEXPECTED, "rating", uuid, "nothing journalled", _describe_rating(now)))
        if ("read_status", uuid) not in claims.slots:
            now_status = state.read_status(uuid)
            if now_status and was.get("read_status") != now_status:
                out.append(_bare(actor, UNEXPECTED, "read status", uuid, "nothing journalled", f"{now_status!r}"))
        if ("journal", uuid) not in claims.slots:
            seen = {normalise_text(j.get("body_md") or "") for j in (was.get("journals") or [])}
            for row in state.journals(uuid):
                body = normalise_text(row.get("body_md") or "")
                if body not in seen:
                    out.append(_bare(actor, UNEXPECTED, "journal entry", uuid, "nothing journalled", body[:80]))

    if claims.shelf_any:
        # A shelf write nothing could attribute to a name: any shelf-level
        # surprise may be it, so the sweep has nothing sound to report.
        return out
    shelves_before = set(before.get("shelves") or [])
    for shelf in state.shelves():
        name = shelf.get("name")
        if name not in shelves_before and name not in claims.shelf_names:
            out.append(_bare(actor, UNEXPECTED, "shelf", None, "nothing journalled", f"shelf {name!r}"))
    return out


def _bare(actor: str, kind: str, what: str, target: str | None, expected: str, observed: str) -> Finding:
    return Finding(actor, None, kind, what, target, expected, observed, None)
