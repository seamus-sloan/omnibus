"""Turn journal entries into the concrete things the audit will look for.

Three jobs live here.

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
add/remove. A remove or edit supersedes only an expectation it can *name* —
by prior text, note, quote, or label — and an unambiguous single occupant when
it names nothing; matching nothing cancels nothing, because a guessed pop
silently un-audits a write that landed.

**Claims.** Alongside the checkable expectations, every entry records which
state slots it addressed — judged or not. `unexpected` findings subtract
these: a write the audit declined to judge usually still landed, and
re-reporting it as "state nothing journalled" would contradict the very
journal line that names it.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field, replace
from typing import Any, Callable, Iterable

from . import vocabulary
from .journal import Entry

UNPARSED = object()
MISSING = object()

# Families whose journal entries restate one value; the last one wins.
SCALAR_FAMILIES = frozenset({"rating", "read_status", "progress", "playback_rate", "book_add"})

# Nouns whose surfaces auto-write read status client-side (unread → reading on
# open, → finished at the end): frontend/src/read_status_auto.rs, mounted by
# the reader, comic reader, and listen pages. Any entry about these nouns
# claims the book's read-status slot, or every book an agent merely opened
# comes back as an `unexpected read status 'reading'`.
AUTO_READ_STATUS_NOUNS = frozenset({"reader", "player", "book"})

# Where an unrecognised action's noun still names the audited slot it touched.
UNKNOWN_NOUN_FAMILY = {"rating": "rating", "status": "read_status", "journal": "journal"}

# The highlight palette as `shared/src/highlight.rs` spells it, plus the
# everyday names an agent reaches for. Anything else is unparsed, not a
# mismatch: "yellowish" is not evidence about the stored colour.
HIGHLIGHT_COLOURS = ("amber", "green", "blue", "rose", "violet")
COLOUR_SYNONYMS = {
    "yellow": "amber",
    "orange": "amber",
    "gold": "amber",
    "pink": "rose",
    "red": "rose",
    "purple": "violet",
    "lilac": "violet",
    "indigo": "blue",
    "teal": "green",
}

# The surface whose journal may name a book by title instead of uuid.
TITLE_KEYED_SURFACE = "ios"


@dataclass
class Claims:
    """The state slots a journal addressed, whether or not each was judged."""

    slots: set = field(default_factory=set)  # (family, uuid)
    shelf_names: set = field(default_factory=set)  # shelf names addressed
    shelf_any: bool = False  # a shelf write nothing could attribute to a name


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
# "did not finish" must never read as finished.
_NOT_FINISHED = re.compile(r"(didn'?t|did\s+not|never|couldn'?t|won'?t|not|un)[\s-]*finish", re.I)


def _half_star(stars: float) -> Any:
    """The UI rates in 0.5 steps to 5; anything else is a misread, not a rating."""
    if not 0 < stars <= 5:
        return UNPARSED
    return stars if abs(stars * 2 - round(stars * 2)) < 1e-6 else UNPARSED


def parse_rating(value: Any) -> Any:
    """A star rating, or `None` for an explicit clear."""
    if value is None:
        return None
    if isinstance(value, bool):
        return UNPARSED
    if isinstance(value, (int, float)):
        return _half_star(float(value))
    if isinstance(value, str):
        if _CLEARED.search(value):
            return None
        m = _NUMBER.search(value)
        if m:
            return _half_star(float(m.group()))
    return UNPARSED


def parse_status(value: Any) -> Any:
    """A read status token, normalised to the wire form."""
    if not isinstance(value, str):
        return UNPARSED
    text = value.strip().lower()
    if "finish" in text and not _NOT_FINISHED.search(text):
        return "finished"
    if "read it" in text:
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


def parse_cfi(value: Any) -> Any:
    """A machine location. Prose ("chapter 4, para 12") is not one."""
    if isinstance(value, str) and "epubcfi(" in value:
        return value.strip()
    return UNPARSED


_COLOUR_WORD = re.compile(r"[a-z]+")


def parse_colour(value: Any) -> Any:
    """A palette token, pulled out of prose.

    `"amber (the reader's default)"` is amber. When the prose names two, the
    last one is taken — "changed from green to violet" describes what was
    left behind, which is what the audit checks (#2362).
    """
    if not isinstance(value, str):
        return UNPARSED
    found = UNPARSED
    for word in _COLOUR_WORD.findall(value.lower()):
        if word in HIGHLIGHT_COLOURS:
            found = word
        elif word in COLOUR_SYNONYMS:
            found = COLOUR_SYNONYMS[word]
    return found


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


def _texts_relate(a: str, b: str) -> bool:
    """Equal or one contains the other, whitespace-normalised."""
    a, b = normalise_text(a), normalise_text(b)
    return bool(a) and bool(b) and (a == b or a in b or b in a)


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
            _get(p, "final_position_app", "final_position_claimed", "final_position_human", "final_position_display"),
            _get(p, "app_shows", "app_position", "location_human"),
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


def _journal_before(entry: Entry) -> Any:
    return _first_parsable(
        (_get(entry.params, "before_verbatim", "before", "old_verbatim", "previous_verbatim"),),
        parse_text,
    )


def _highlight_keys(entry: Entry) -> dict[str, Any]:
    p = entry.params
    note = _first_parsable((_get(p, "note_text", "note", "annotation"),), parse_text)
    quote = _first_parsable((_get(p, "text", "quote", "selected_text", "passage"),), parse_text)
    colour = _first_parsable(
        (
            _get(p, "new_colour", "new_color", "to_colour", "to_color"),
            _get(p, "colour", "color"),
            _get(p, "to"),
        ),
        parse_colour,
    )
    cfi = _first_parsable((_get(p, "epub_cfi_range", "cfi_range", "epub_cfi", "cfi", "location"),), parse_cfi)
    return {
        "note": None if note is UNPARSED else note,
        "quote": None if quote is UNPARSED else quote,
        "colour": None if colour is UNPARSED else colour,
        "cfi": None if cfi is UNPARSED else cfi,
    }


def _old_note(entry: Entry) -> Any:
    return _first_parsable(
        (_get(entry.params, "old_note", "previous_note", "note_before", "before_note", "old"),),
        parse_text,
    )


def _old_colour(entry: Entry) -> Any:
    """The colour a recolour moved away from — how it names its prior."""
    return _first_parsable(
        (
            _get(entry.params, "old_colour", "old_color", "from_colour", "from_color"),
            _get(entry.params, "previous_colour", "previous_color", "from"),
        ),
        parse_colour,
    )


def _bookmark_keys(entry: Entry) -> dict[str, Any]:
    p = entry.params
    label = _first_parsable((_get(p, "title", "label", "name"),), parse_text)
    position = _first_parsable((_get(p, "position", "epub_cfi", "cfi", "location"),), parse_text)
    return {
        "label": None if label is UNPARSED else label,
        "position": None if position is UNPARSED else position,
    }


def _name(entry: Entry) -> Any:
    return _first_parsable((_get(entry.params, "name", "shelf_name", "title"),), parse_text)


def _uuid(entry: Entry) -> Any:
    for candidate in (entry.target, _get(entry.params, "uuid", "book_uuid", "target")):
        if isinstance(candidate, str) and candidate.strip():
            return candidate.strip()
    return UNPARSED


def _no_uuid(entry: Entry) -> str:
    why = f"{entry.action}: no book uuid on the entry or in params"
    return f"{why} — {entry.target_note}" if entry.target_note else why


def _title(entry: Entry) -> Any:
    return _first_parsable((_get(entry.params, "title", "book_title", "book"),), parse_text)


def resolve_targets(
    entries: list[Entry], resolve: Callable[[str], tuple[str | None, str]]
) -> list[Entry]:
    """Fill a null `target` on an iOS entry from the title it names.

    The native app has no screen that shows a uuid (#2365), so that lane's
    entries carry the book's exact title in `params.title` instead and the
    audit resolves it against the library. Only the `ios` surface gets this:
    a web agent has the uuid on the page, and resolving for it would hide a
    journal that broke the contract. A title that resolves to nothing, or to
    more than one book, leaves the target null with the reason attached, so
    the entry is declined for a stated cause rather than a bare "no uuid".
    """
    out: list[Entry] = []
    for entry in entries:
        if entry.surface != TITLE_KEYED_SURFACE or _uuid(entry) is not UNPARSED:
            out.append(entry)
            continue
        title = _title(entry)
        if title is UNPARSED:
            out.append(entry)
            continue
        uuid, why = resolve(str(title))
        out.append(replace(entry, target=uuid) if uuid else replace(entry, target_note=why))
    return out


# --- folding ---------------------------------------------------------------


class _Fold:
    """Accumulates one actor's expectations, family by family."""

    def __init__(self, actor: str) -> None:
        self.actor = actor
        self.scalars: dict[tuple, Expectation] = {}
        self.sets: dict[tuple[str, str | None], list[Expectation]] = {}
        self.unverifiable: list[Unverifiable] = []

    def skip(self, entry: Entry, why: str) -> None:
        self.unverifiable.append(Unverifiable(self.actor, entry.seq, why))

    def scalar(self, exp: Expectation, key: tuple | None = None) -> None:
        self.scalars[key or (exp.family, exp.target)] = exp

    def push(self, exp: Expectation) -> None:
        self.sets.setdefault((exp.family, exp.target), []).append(exp)

    def pop(
        self, family: str, target: str | None, match: Callable[[Expectation], bool] | None = None
    ) -> Expectation | None:
        """Drop and return the expectation a delete/edit supersedes.

        A predicate that matches nothing removes **nothing**. Falling back to
        "drop the most recent" looks harmless and is not: an agent removing a
        wishlist entry an *earlier* run added would silently cancel the
        expectation for the book it added in *this* one, and the audit would
        never look for it.
        """
        bucket = self.sets.get((family, target))
        if not bucket:
            return None
        if match is not None:
            for i in range(len(bucket) - 1, -1, -1):
                if match(bucket[i]):
                    return bucket.pop(i)
            return None
        return bucket.pop()

    def pop_single(self, family: str, target: str | None) -> Expectation | None:
        """Pop the bucket's only occupant; ambiguity pops nothing.

        The fallback when a delete/edit names nothing identifying: with one
        candidate the reference is unambiguous, with two it would be a guess,
        and a guessed pop cancels an expectation for a write that landed.
        """
        bucket = self.sets.get((family, target))
        if bucket and len(bucket) == 1:
            return bucket.pop()
        return None

    def result(self) -> tuple[list[Expectation], list[Unverifiable]]:
        out = list(self.scalars.values())
        for bucket in self.sets.values():
            out.extend(bucket)
        out.sort(key=lambda e: (e.seq is None, e.seq or 0))
        return out, self.unverifiable


def _ann_match(want: dict[str, Any]) -> Callable[[Expectation], bool]:
    """Predicate matching a folded annotation on the identifying values in `want`."""

    def matches(e: Expectation) -> bool:
        prior = e.value if isinstance(e.value, dict) else {}
        for key, wanted in want.items():
            held = prior.get(key)
            if held is None or not _texts_relate(str(held), str(wanted)):
                return False
        return True

    return matches


def _fold_entry(fold: _Fold, entry: Entry, cls: vocabulary.Classification) -> None:
    family, detail = cls.family, cls.detail
    p = entry.params
    seq, actor = entry.seq, fold.actor

    def exp(what: str, target: str | None, expected: str, value: Any = None, **extra: Any) -> Expectation:
        return Expectation(actor, seq, family or "", what, target, expected, value, extra)

    if family in SCALAR_FAMILIES:
        uuid = _uuid(entry)
        if uuid is UNPARSED:
            fold.skip(entry, _no_uuid(entry))
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
            if pos["seconds"] is None and pos["percent"] is None and pos["cfi"] is None:
                # A bare open-and-close may never have written a position
                # server-side (positions save on movement), so an
                # exists-assertion here would be the audit's own false
                # positive. Superseding nothing keeps any earlier, positioned
                # entry's expectation for this book alive.
                fold.skip(entry, f"{entry.action}: no position recorded in params — cannot assert one was written")
                return
            axis = "listening" if pos["axis"] == "audio" else "reading"
            where = []
            if pos["seconds"] is not None:
                where.append(f"{pos['seconds']:.0f}s")
            if pos["percent"] is not None:
                where.append(f"{pos['percent']:g}%")
            detail_text = f" near {', '.join(where)}" if where else ""
            # Keyed per axis: a dual-format book holds one row per format, and
            # folding reader and player statements together would leave one
            # axis never checked.
            fold.scalar(
                exp("progress", uuid, f"a saved {axis} position{detail_text}", pos),
                key=("progress", uuid, pos["axis"]),
            )
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
            fold.skip(entry, _no_uuid(entry))
            return
        if detail == "delete":
            text = _journal_text(entry)
            popped = (
                fold.pop("journal", uuid, lambda e: _texts_relate(str(e.value), str(text)))
                if text is not UNPARSED
                else fold.pop_single("journal", uuid)
            )
            if popped is None:
                fold.skip(entry, f"{entry.action}: removed a journal entry this run did not create")
            return
        text = _journal_text(entry)
        if text is UNPARSED:
            fold.skip(entry, f"{entry.action}: no entry text in params (expected entry_text_verbatim)")
            return
        phrase = _first_parsable((_get(p, "distinctive_phrase"),), parse_text)
        if detail == "update":
            # Supersede the entry it *edited* — named by its prior text — not
            # whichever entry happens to be most recent on the book.
            before = _journal_before(entry)
            if before is not UNPARSED:
                fold.pop("journal", uuid, lambda e: _texts_relate(str(e.value), str(before)))
            else:
                fold.pop_single("journal", uuid)
            # Matching nothing cancels nothing: the edit was of an earlier
            # run's entry, and the new text is still asserted below.
        fold.push(
            exp(
                "journal entry",
                uuid,
                f"journal entry containing {normalise_text(text)[:60]!r}…",
                normalise_text(text),
                phrase=None if phrase is UNPARSED else phrase,
            )
        )
        return

    if family in ("highlight", "bookmark"):
        uuid = _uuid(entry)
        if uuid is UNPARSED:
            fold.skip(entry, _no_uuid(entry))
            return
        noun = family
        keys = _highlight_keys(entry) if family == "highlight" else _bookmark_keys(entry)
        ident = {k: v for k, v in keys.items() if v is not None and k in ("note", "quote", "label", "position")}
        if detail == "delete":
            popped = fold.pop(family, uuid, _ann_match(ident)) if ident else fold.pop_single(family, uuid)
            if popped is None:
                fold.skip(entry, f"{entry.action}: removed a {noun} this run did not create")
            return
        if detail == "update":
            # Find the prior by what it *was*: the old note when journalled,
            # else a key the edit does not change (the quote, the label).
            old = _old_note(entry)
            if old is not UNPARSED:
                want: dict[str, Any] = {"note": old}
            else:
                want = {k: v for k, v in ident.items() if k in ("quote", "label", "position")}
                old_colour = _old_colour(entry) if family == "highlight" else UNPARSED
                if old_colour is not UNPARSED:
                    want["colour"] = old_colour
            popped = fold.pop(family, uuid, _ann_match(want)) if want else fold.pop_single(family, uuid)
            if popped is None:
                # An edit the fold cannot attribute must not be guessed onto a
                # neighbour, and asserting only the edited field would be too
                # weak to mean anything. Decline to judge it.
                fold.skip(entry, f"{entry.action}: edited a {noun} this run cannot attribute — nothing superseded")
                return
            merged = dict(popped.value) if isinstance(popped.value, dict) else {}
            merged.update({k: v for k, v in keys.items() if v is not None})
            keys = merged
        label = keys.get("note") or keys.get("quote") or keys.get("label")
        described = f"{noun} on {uuid}" + (f" with {str(label)[:50]!r}" if label else "")
        fold.push(exp(noun, uuid, described, keys))
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
            fold.skip(entry, _no_uuid(entry))
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


def _claim(claims: Claims, entry: Entry, cls: vocabulary.Classification) -> None:
    """Record which slots this entry addressed, whatever became of it."""
    uuid = _uuid(entry)
    noun = vocabulary.noun_of(entry.action or "")
    # The reading surfaces auto-write read status with nothing journalled
    # (read_status_auto.rs), so any entry about them claims that slot.
    if noun in AUTO_READ_STATUS_NOUNS and uuid is not UNPARSED:
        claims.slots.add(("read_status", uuid))

    if cls.kind == vocabulary.WRITE:
        if cls.family == "shelf":
            name = _name(entry)
            if name is UNPARSED:
                claims.shelf_any = True
            else:
                claims.shelf_names.add(name)
        elif uuid is not UNPARSED:
            claims.slots.add((cls.family or "", uuid))
        return

    if cls.kind == vocabulary.UNKNOWN and noun:
        if noun == "shelf":
            name = _name(entry)
            if name is UNPARSED:
                claims.shelf_any = True
            else:
                claims.shelf_names.add(name)
        family = UNKNOWN_NOUN_FAMILY.get(noun)
        if family and uuid is not UNPARSED:
            claims.slots.add((family, uuid))


def expectations_for(
    actor: str, entries: list[Entry]
) -> tuple[list[Expectation], list[Unverifiable], dict[str, int], Claims]:
    """Fold one actor's ordered entries into what should be true now.

    Returns the expectations, the entries the audit declined to judge, a
    tally of classifications, and the `Claims` the journal made on state —
    judged or not — which is what `unexpected` findings subtract.
    """
    fold = _Fold(actor)
    claims = Claims()
    tally: dict[str, int] = {}
    for entry in entries:
        cls = vocabulary.classify(entry.action)
        tally[cls.kind] = tally.get(cls.kind, 0) + 1
        _claim(claims, entry, cls)
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
    return exps, unver, tally, claims
