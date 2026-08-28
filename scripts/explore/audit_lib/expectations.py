"""Turn journal entries into the concrete things the audit will look for.

Two jobs live here.

**Extraction.** `params` is free-form by design — `start.md` asks for
"everything a replayer needs", not a schema — so every value is pulled from a
list of candidate spellings and each candidate must *parse* before it is
accepted. A key that is present but holds something else (`left_in_this_state:
true` sitting where a status was expected) falls through to the next candidate
rather than poisoning the check. When nothing parses the entry becomes
`Unverifiable` — never a finding, because "I could not read the intent" is not
evidence that the write went missing.

**Folding.** An agent restates the same value repeatedly: three `rating.set`
entries on one book are one final rating, and `wishlist.add` followed by
`wishlist.remove` expects nothing at all. Superseded entries are not findings,
so each family folds its ordered entries down to what should be true *now*:
scalar families keep the last parseable statement, set families replay
add/remove.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable

from . import vocabulary
from .journal import Entry

UNPARSED = object()
MISSING = object()

# Families whose journal entries restate one value; the last one wins.
SCALAR_FAMILIES = frozenset({"rating", "read_status", "progress", "playback_rate", "book_add"})


@dataclass
class Expectation:
    """One thing the audit will look for in server state."""

    actor: str
    seq: int | None
    family: str
    what: str
    target: str | None
    expected: str
    value: Any = None
    extra: dict[str, Any] = field(default_factory=dict)


@dataclass
class Unverifiable:
    """A journal entry the audit deliberately does not judge, and why."""

    actor: str
    seq: int | None
    why: str


def _get(params: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        if key in params:
            return params[key]
    return MISSING


def _last_of(params: dict[str, Any], list_key: str, *keys: str) -> Any:
    seq = params.get(list_key)
    if isinstance(seq, list) and seq and isinstance(seq[-1], dict):
        return _get(seq[-1], *keys)
    return MISSING


def _first_parsable(candidates: Iterable[Any], parse: Callable[[Any], Any]) -> Any:
    for candidate in candidates:
        if candidate is MISSING:
            continue
        parsed = parse(candidate)
        if parsed is not UNPARSED:
            return parsed
    return UNPARSED


# --- value parsers ---------------------------------------------------------

_NUMBER = re.compile(r"-?\d+(?:\.\d+)?")
_CLEARED = re.compile(r"\b(cleared|clear|none|unrated|not rated|null|removed)\b", re.I)
_PERCENT = re.compile(r"(\d+(?:\.\d+)?)\s*%")


def parse_rating(value: Any) -> Any:
    """A star rating, or `None` for an explicit clear."""
    if value is None:
        return None
    if isinstance(value, bool):
        return UNPARSED
    if isinstance(value, (int, float)):
        return float(value) if 0 < float(value) <= 5 else UNPARSED
    if isinstance(value, str):
        if _CLEARED.search(value):
            return None
        m = _NUMBER.search(value)
        if m:
            stars = float(m.group())
            if 0 < stars <= 5:
                return stars
    return UNPARSED


def parse_status(value: Any) -> Any:
    """A read status token, normalised to the wire form."""
    if not isinstance(value, str):
        return UNPARSED
    text = value.strip().lower()
    if "finish" in text or "read it" in text:
        return "finished"
    if "in progress" in text or "reading" in text or "started" in text and "not started" not in text:
        return "reading"
    if "unread" in text or "not started" in text or "want" in text:
        return "unread"
    return UNPARSED


def parse_seconds(value: Any) -> Any:
    """A media position in seconds."""
    if isinstance(value, bool):
        return UNPARSED
    if isinstance(value, (int, float)):
        return float(value) if value >= 0 else UNPARSED
    if isinstance(value, str):
        m = _NUMBER.search(value)
        if m and float(m.group()) >= 0:
            return float(m.group())
    return UNPARSED


def parse_rate(value: Any) -> Any:
    """A playback rate; `1.20x` and `1.2` are the same thing."""
    if isinstance(value, bool):
        return UNPARSED
    if isinstance(value, (int, float)):
        return float(value) if 0.1 <= float(value) <= 5 else UNPARSED
    if isinstance(value, str):
        m = _NUMBER.search(value)
        if m and 0.1 <= float(m.group()) <= 5:
            return float(m.group())
    return UNPARSED


def parse_text(value: Any) -> Any:
    """Non-empty prose."""
    if isinstance(value, str) and value.strip():
        return value
    return UNPARSED


def parse_percent(value: Any) -> Any:
    if isinstance(value, bool):
        return UNPARSED
    if isinstance(value, (int, float)) and 0 <= float(value) <= 100:
        return float(value)
    if isinstance(value, str):
        m = _PERCENT.search(value)
        if m:
            return float(m.group(1))
    return UNPARSED


def normalise_text(text: str) -> str:
    """Collapse whitespace so a re-render's line wrapping is not a mismatch."""
    return re.sub(r"\s+", " ", text).strip()


# --- per-entry extraction --------------------------------------------------


def _rating(entry: Entry) -> Any:
    p = entry.params
    return _first_parsable(
        (
            _get(p, "final_rating_left_behind", "final_rating", "final"),
            _last_of(p, "sequence", "new", "new_rating", "to"),
            _last_of(p, "transitions", "new", "new_rating", "to"),
            _get(p, "new", "new_rating", "new_stars", "stars", "rating", "value", "to"),
        ),
        parse_rating,
    )


def _status(entry: Entry) -> Any:
    p = entry.params
    return _first_parsable(
        (
            _last_of(p, "transitions", "new", "new_status", "to"),
            _last_of(p, "sequence", "new", "new_status", "to"),
            _get(p, "left_in_this_state", "left_at", "final", "final_status"),
            _get(p, "new", "new_status", "status", "to"),
        ),
        parse_status,
    )


def _rate(entry: Entry) -> Any:
    p = entry.params
    return _first_parsable(
        (
            _get(p, "rate_after_return", "final_rate", "new_rate", "rate_after"),
            _get(p, "to_rate", "rate", "playbackRate"),
        ),
        parse_rate,
    )


AUDIO_ACTIONS = ("player", "listen", "audio")


def _progress_axis(entry: Entry) -> str:
    """Which position axis this entry's write lands on."""
    fmt = entry.params.get("format")
    if isinstance(fmt, str):
        if fmt.lower() in ("m4b", "m4a", "mp3", "audio", "audiobook"):
            return "audio"
        if fmt.lower() in ("epub", "cbz", "ebook"):
            return "ebook"
    head = vocabulary.normalise(entry.action or "")[:1]
    return "audio" if head and head[0] in AUDIO_ACTIONS else "ebook"


def _progress(entry: Entry) -> dict[str, Any]:
    p = entry.params
    axis = _progress_axis(entry)
    seconds = _first_parsable(
        (
            _get(p, "final_position_secs", "final_position_seconds"),
            _get(p, "to_secs", "to_seconds", "position_seconds", "position_secs"),
            _get(p, "paused_at_secs", "returned_position_secs"),
        ),
        parse_seconds,
    )
    percent = _first_parsable(
        (
            _get(p, "progress_percent", "percent"),
            _get(p, "app_shows", "app_position", "final_position_app", "location_human"),
        ),
        parse_percent,
    )
    cfi = _first_parsable((_get(p, "epub_cfi", "cfi", "epubcfi"),), parse_text)
    return {
        "axis": axis,
        "seconds": None if seconds is UNPARSED else seconds,
        "percent": None if percent is UNPARSED else percent,
        "cfi": None if cfi is UNPARSED else cfi,
    }


def _journal_text(entry: Entry) -> Any:
    p = entry.params
    return _first_parsable(
        (
            _get(p, "after_verbatim", "after", "new_verbatim"),
            _get(p, "entry_text_verbatim", "body_md", "entry_text", "text", "body"),
        ),
        parse_text,
    )


def _highlight_keys(entry: Entry) -> dict[str, Any]:
    p = entry.params
    note = _first_parsable((_get(p, "note_text", "note", "annotation"),), parse_text)
    quote = _first_parsable((_get(p, "text", "quote", "selected_text", "passage"),), parse_text)
    colour = _first_parsable((_get(p, "colour", "color"),), parse_text)
    return {
        "note": None if note is UNPARSED else note,
        "quote": None if quote is UNPARSED else quote,
        "colour": None if colour is UNPARSED else colour,
    }


def _name(entry: Entry) -> Any:
    return _first_parsable((_get(entry.params, "name", "shelf_name", "title"),), parse_text)


def _uuid(entry: Entry) -> Any:
    for candidate in (entry.target, _get(entry.params, "uuid", "book_uuid", "target")):
        if isinstance(candidate, str) and candidate.strip():
            return candidate.strip()
    return UNPARSED


# --- folding ---------------------------------------------------------------


class _Fold:
    """Accumulates one actor's expectations, family by family."""

    def __init__(self, actor: str) -> None:
        self.actor = actor
        self.scalars: dict[tuple[str, str | None], Expectation] = {}
        self.sets: dict[tuple[str, str | None], list[Expectation]] = {}
        self.unverifiable: list[Unverifiable] = []

    def skip(self, entry: Entry, why: str) -> None:
        self.unverifiable.append(Unverifiable(self.actor, entry.seq, why))

    def scalar(self, exp: Expectation) -> None:
        self.scalars[(exp.family, exp.target)] = exp

    def push(self, exp: Expectation) -> None:
        self.sets.setdefault((exp.family, exp.target), []).append(exp)

    def pop(self, family: str, target: str | None, match: Callable[[Expectation], bool] | None = None) -> bool:
        """Drop the expectation a delete/remove supersedes; `False` if none.

        A predicate that matches nothing must remove **nothing**. Falling back
        to "drop the most recent" looks harmless and is not: an agent removing
        a wishlist entry an *earlier* run added would silently cancel the
        expectation for the book it added in *this* one, and the audit would
        never look for it.
        """
        bucket = self.sets.get((family, target))
        if not bucket:
            return False
        if match is not None:
            for i in range(len(bucket) - 1, -1, -1):
                if match(bucket[i]):
                    bucket.pop(i)
                    return True
            return False
        bucket.pop()
        return True

    def result(self) -> tuple[list[Expectation], list[Unverifiable]]:
        out = list(self.scalars.values())
        for bucket in self.sets.values():
            out.extend(bucket)
        out.sort(key=lambda e: (e.seq is None, e.seq or 0))
        return out, self.unverifiable


def _fold_entry(fold: _Fold, entry: Entry, cls: vocabulary.Classification) -> None:
    family, detail = cls.family, cls.detail
    p = entry.params
    seq, actor = entry.seq, fold.actor

    def exp(what: str, target: str | None, expected: str, value: Any = None, **extra: Any) -> Expectation:
        return Expectation(actor, seq, family or "", what, target, expected, value, extra)

    if family in ("rating", "read_status", "progress", "playback_rate", "book_add"):
        uuid = _uuid(entry)
        if uuid is UNPARSED:
            fold.skip(entry, f"{entry.action}: no book uuid on the entry or in params")
            return
        if family == "rating":
            stars = _rating(entry)
            if stars is UNPARSED:
                fold.skip(entry, f"{entry.action}: no readable rating in params")
                return
            fold.scalar(exp("rating", uuid, "no rating" if stars is None else f"rating {stars:g} of 5", stars))
        elif family == "read_status":
            status = _status(entry)
            if status is UNPARSED:
                fold.skip(entry, f"{entry.action}: no readable read status in params")
                return
            fold.scalar(exp("read status", uuid, f"read status {status!r}", status))
        elif family == "progress":
            pos = _progress(entry)
            axis = "listening" if pos["axis"] == "audio" else "reading"
            where = []
            if pos["seconds"] is not None:
                where.append(f"{pos['seconds']:.0f}s")
            if pos["percent"] is not None:
                where.append(f"{pos['percent']:g}%")
            detail_text = f" near {', '.join(where)}" if where else ""
            fold.scalar(exp("progress", uuid, f"a saved {axis} position{detail_text}", pos))
        elif family == "playback_rate":
            rate = _rate(entry)
            if rate is UNPARSED:
                fold.skip(entry, f"{entry.action}: no readable playback rate in params")
                return
            fold.scalar(exp("playback rate", uuid, f"playback rate {rate:g}x", rate))
        else:
            fold.scalar(exp("book", uuid, f"book {uuid} present in the library", uuid))
        return

    if family == "journal":
        uuid = _uuid(entry)
        if uuid is UNPARSED:
            fold.skip(entry, f"{entry.action}: no book uuid on the entry or in params")
            return
        if detail == "delete":
            if not fold.pop("journal", uuid):
                fold.skip(entry, f"{entry.action}: removed a journal entry this run did not create")
            return
        text = _journal_text(entry)
        if text is UNPARSED:
            fold.skip(entry, f"{entry.action}: no entry text in params (expected entry_text_verbatim)")
            return
        phrase = _first_parsable((_get(p, "distinctive_phrase"),), parse_text)
        new = exp(
            "journal entry",
            uuid,
            f"journal entry containing {normalise_text(text)[:60]!r}…",
            normalise_text(text),
            phrase=None if phrase is UNPARSED else phrase,
        )
        if detail == "update":
            fold.pop("journal", uuid)
        fold.push(new)
        return

    if family in ("highlight", "bookmark"):
        uuid = _uuid(entry)
        if uuid is UNPARSED:
            fold.skip(entry, f"{entry.action}: no book uuid on the entry or in params")
            return
        keys = _highlight_keys(entry) if family == "highlight" else {}
        matches = (lambda e: e.value.get("note") == keys.get("note")) if keys.get("note") else None
        if detail == "delete":
            if not fold.pop(family, uuid, matches):
                fold.skip(entry, f"{entry.action}: removed an annotation this run did not create")
            return
        if detail == "update":
            fold.pop(family, uuid, matches)
        label = keys.get("note") or keys.get("quote") if keys else None
        what = "highlight" if family == "highlight" else "bookmark"
        described = f"{what} on {uuid}" + (f" with {label[:50]!r}" if label else "")
        fold.push(exp(what, uuid, described, keys))
        return

    if family == "shelf":
        name = _name(entry)
        if name is UNPARSED:
            fold.skip(entry, f"{entry.action}: no shelf name in params")
            return
        if detail == "delete":
            if not fold.pop("shelf", None, lambda e: e.value == name):
                fold.skip(entry, f"{entry.action}: deleted a shelf this run did not create")
            return
        fold.push(exp("shelf", None, f"a shelf named {name!r}", name))
        return

    if family == "shelf_member":
        uuid = _uuid(entry)
        shelf = _name(entry)
        if uuid is UNPARSED or shelf is UNPARSED:
            fold.skip(entry, f"{entry.action}: needs both a shelf name and a book uuid in params")
            return
        if detail == "remove":
            if not fold.pop("shelf_member", shelf, lambda e: e.value == uuid):
                fold.skip(entry, f"{entry.action}: removed a shelf member this run did not add")
            return
        fold.push(exp("shelf membership", shelf, f"{uuid} on shelf {shelf!r}", uuid))
        return

    if family == "wishlist":
        uuid = _uuid(entry)
        if uuid is UNPARSED:
            fold.skip(entry, f"{entry.action}: no book uuid on the entry or in params")
            return
        if detail == "remove":
            if not fold.pop("wishlist", None, lambda e: e.value == uuid):
                fold.skip(entry, f"{entry.action}: removed a wishlist entry this run did not add")
            return
        title = _first_parsable((_get(p, "chosen_title", "title"),), parse_text)
        label = uuid if title is UNPARSED else f"{title} ({uuid})"
        fold.push(exp("wishlist entry", None, f"{label} on the wishlist", uuid))
        return

    fold.skip(entry, f"{entry.action}: classified as a write but no extractor — audit bug")


def expectations_for(actor: str, entries: list[Entry]) -> tuple[list[Expectation], list[Unverifiable], dict[str, int]]:
    """Fold one actor's ordered entries into what should be true now.

    Returns the expectations, the entries the audit declined to judge, and a
    tally of how each entry was classified.
    """
    fold = _Fold(actor)
    tally: dict[str, int] = {}
    for entry in entries:
        cls = vocabulary.classify(entry.action)
        tally[cls.kind] = tally.get(cls.kind, 0) + 1
        if cls.kind == vocabulary.OBSERVATION:
            continue
        if cls.kind == vocabulary.OUT_OF_SCOPE:
            fold.skip(entry, f"{entry.action}: {cls.detail}")
            continue
        if cls.kind == vocabulary.UNKNOWN:
            fold.skip(entry, cls.reason or f"{entry.action}: unclassified")
            continue
        if entry.outcome != "ok":
            fold.skip(entry, f"{entry.action}: outcome={entry.outcome!r} — not a completed write")
            continue
        _fold_entry(fold, entry, cls)
    exps, unver = fold.result()
    return exps, unver, tally
