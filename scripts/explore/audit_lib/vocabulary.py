"""Classify a journal `action` name into something the audit can act on.

`start.md` gives action names as *examples*, not an enum, and two runs of
free-running agents produced **52 distinct names across 197 entries — 27 of
them used exactly once**. Nine verbs on `player`, six on `book`, six on
`reader`. An audit keyed on exact strings would silently skip the writes it
did not recognise, which is the worst failure an audit has: it reports
success.

So the vocabulary is **a closed list of nouns with a conventioned verb slot**,
not a closed list of names. A name is `noun.verb`, lowercase, and the noun is
what decides how much freedom the verb gets:

* **Nouns that carry no audited state** (`nav`, `search`, `auth`, `stats`, …)
  take *any* verb and classify as `OBSERVATION`. Nothing can be missed,
  because there is nothing there to miss.
* **Nouns excluded by policy** (`metadata`, `cover`, `merge`, `profile`, …)
  take any verb and classify as `OUT_OF_SCOPE` with the reason quoted into
  `audit.json`.
* **Nouns that carry audited state** (`rating`, `reader`, `journal`, …) take
  only listed verbs, because the verb is what decides whether the audit
  should assert presence or absence — guessing that backwards invents
  findings. An unlisted verb on such a noun is `UNKNOWN`.

`UNKNOWN` is never dropped: it lands in `audit.json`'s `unverifiable` list
naming the action, so a newly invented verb is a visible gap rather than
silent coverage. `audit.py vocab` lists them, and each one costs exactly one
line here.

Two rules dissolve the drift the measurement found before any lookup happens:

1. **Separator and depth drift.** Names split on `.`, `_`, `-` and lowercase,
   so `reader.resume_check` and `reader.resume.check` are one thing.
2. **The `.verify` suffix.** Agents kept inventing `book.add.verify`,
   `journal.persist.verify`, `reader.resume.verify` — they needed a way to
   say "I checked it stuck" and the contract offered none. A trailing
   qualifier segment therefore means observation on *any* noun, by rule
   rather than by enumeration.

The one soft edge: when a name has more than two segments and its **last**
segment is a listed *non-write* verb for the noun, that classification is
accepted (`book.detail.open` → `book.open`). Writes are never resolved this
way — a write must match `noun.verb` exactly — so the fallback can mislabel a
look as a look, and nothing worse.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

WRITE = "write"
OBSERVATION = "observation"
OUT_OF_SCOPE = "out_of_scope"
UNKNOWN = "unknown"

SPLIT = re.compile(r"[.\-_\s]+")

# A trailing segment meaning "I looked / I checked it stuck", on any noun.
QUALIFIERS = frozenset(
    {
        "verify", "verified", "verification", "check", "checked", "recheck",
        "attempt", "attempted", "try", "view", "viewed", "list", "listed",
        "read", "inspect", "observe", "observed", "assert", "expect",
    }
)

# Segments that only ever qualify the one before them, dropped before the
# qualifier test — `journal.persist.verify` is `journal` + "I checked".
FILLER = frozenset({"persist", "persisted", "state", "again", "post", "after"})

# Scope reasons, quoted verbatim into `unverifiable[].why`.
SCOPE_METADATA = "metadata override — out of audit scope"
SCOPE_LIBRARY = "library-wide edit, no single intent owns the value — out of audit scope"
SCOPE_ACCOUNT = "account configuration — not in the audited per-user state set"

# Spelling variants of one noun. Plurals and the two spellings of read-status
# are drift, not meaning.
NOUN_ALIASES = {
    "ratings": "rating",
    "shelves": "shelf",
    "readstatus": "status",
    "highlights": "highlight",
    "annotation": "highlight",
    "annotations": "highlight",
    "bookmarks": "bookmark",
    "journals": "journal",
    "listen": "player",
    "audio": "player",
    "audiobook": "player",
    "playback": "player",
    "reading": "reader",
    "books": "book",
    "genres": "genre",
    "tags": "tag",
    "subjects": "genre",
    "override": "metadata",
    "overrides": "metadata",
    "physical": "checkin",
    "unmerge": "merge",
    "account": "profile",
    "avatar": "profile",
}

# What an unlisted verb means, per noun. `UNKNOWN` is the deliberate choice
# for every noun that can carry state the audit reads back.
NOUN_POLICY = {
    # Audited state — the verb decides presence vs absence, so it must be listed.
    "rating": UNKNOWN,
    "status": UNKNOWN,
    "progress": UNKNOWN,
    "reader": UNKNOWN,
    "player": UNKNOWN,
    "journal": UNKNOWN,
    "highlight": UNKNOWN,
    "bookmark": UNKNOWN,
    "shelf": UNKNOWN,
    "wishlist": UNKNOWN,
    "book": UNKNOWN,
    # No audited state on the noun at all — any verb is a look.
    "nav": OBSERVATION,
    "ui": OBSERVATION,
    "search": OBSERVATION,
    "library": OBSERVATION,
    "author": OBSERVATION,
    "series": OBSERVATION,
    "stats": OBSERVATION,
    "suggestions": OBSERVATION,
    "auth": OBSERVATION,
    "session": OBSERVATION,
    "flow": OBSERVATION,
    "page": OBSERVATION,
    # Excluded by policy — any verb on the noun is out of scope.
    "metadata": OUT_OF_SCOPE,
    "cover": OUT_OF_SCOPE,
    "genre": OUT_OF_SCOPE,
    "tag": OUT_OF_SCOPE,
    "merge": OUT_OF_SCOPE,
    "profile": OUT_OF_SCOPE,
    "settings": OUT_OF_SCOPE,
    "checkin": OUT_OF_SCOPE,
    "kindle": OUT_OF_SCOPE,
    "kobo": OUT_OF_SCOPE,
}

# Nouns spelled as two segments. Without this, `read-status.set` splits into
# noun `read` + verb `status_set` and resolves to nothing.
COMPOUND_NOUNS = {
    ("read", "status"): "status",
    ("shelf", "member"): "shelf",
}

SCOPE_FOR_NOUN = {
    "metadata": SCOPE_METADATA,
    "cover": SCOPE_METADATA,
    "genre": SCOPE_METADATA,
    "tag": SCOPE_METADATA,
    "merge": SCOPE_LIBRARY,
    "checkin": SCOPE_LIBRARY,
    "settings": SCOPE_LIBRARY,
    "kindle": SCOPE_LIBRARY,
    "kobo": SCOPE_LIBRARY,
    "profile": SCOPE_ACCOUNT,
}

# (noun, verb) -> (kind, family, detail). Only writes and the non-write verbs
# on stateful nouns need a line; every other noun's policy covers it.
VERBS: dict[tuple[str, str], tuple[str, str | None, str | None]] = {
    ("rating", "set"): (WRITE, "rating", None),
    ("rating", "change"): (WRITE, "rating", None),
    ("rating", "clear"): (WRITE, "rating", None),
    ("status", "set"): (WRITE, "read_status", None),
    ("status", "change"): (WRITE, "read_status", None),
    ("status", "clear"): (WRITE, "read_status", None),
    ("progress", "save"): (WRITE, "progress", None),
    ("progress", "set"): (WRITE, "progress", None),
    ("reader", "progress"): (WRITE, "progress", None),
    ("reader", "close"): (WRITE, "progress", None),
    ("reader", "open"): (OBSERVATION, None, None),
    ("reader", "toc"): (OBSERVATION, None, None),
    ("reader", "settings"): (OBSERVATION, None, None),
    ("player", "seek"): (WRITE, "progress", None),
    ("player", "close"): (WRITE, "progress", None),
    ("player", "progress"): (WRITE, "progress", None),
    ("player", "rate"): (WRITE, "playback_rate", None),
    ("player", "speed"): (WRITE, "playback_rate", None),
    ("player", "open"): (OBSERVATION, None, None),
    ("player", "play"): (OBSERVATION, None, None),
    ("player", "pause"): (OBSERVATION, None, None),
    ("player", "chapters"): (OBSERVATION, None, None),
    ("player", "seam"): (OBSERVATION, None, None),
    ("player", "sleep"): (OBSERVATION, None, None),
    ("player", "volume"): (OBSERVATION, None, None),
    ("journal", "create"): (WRITE, "journal", "create"),
    ("journal", "add"): (WRITE, "journal", "create"),
    ("journal", "write"): (WRITE, "journal", "create"),
    ("journal", "update"): (WRITE, "journal", "update"),
    ("journal", "edit"): (WRITE, "journal", "update"),
    ("journal", "delete"): (WRITE, "journal", "delete"),
    ("journal", "remove"): (WRITE, "journal", "delete"),
    ("highlight", "create"): (WRITE, "highlight", "create"),
    ("highlight", "add"): (WRITE, "highlight", "create"),
    ("highlight", "note"): (WRITE, "highlight", "update"),
    ("highlight", "colour"): (WRITE, "highlight", "update"),
    ("highlight", "color"): (WRITE, "highlight", "update"),
    ("highlight", "delete"): (WRITE, "highlight", "delete"),
    ("highlight", "remove"): (WRITE, "highlight", "delete"),
    ("bookmark", "create"): (WRITE, "bookmark", "create"),
    ("bookmark", "add"): (WRITE, "bookmark", "create"),
    ("bookmark", "delete"): (WRITE, "bookmark", "delete"),
    ("bookmark", "remove"): (WRITE, "bookmark", "delete"),
    ("shelf", "create"): (WRITE, "shelf", "create"),
    ("shelf", "delete"): (WRITE, "shelf", "delete"),
    ("shelf", "add"): (WRITE, "shelf_member", "add"),
    ("shelf", "add_books"): (WRITE, "shelf_member", "add"),
    ("shelf", "remove"): (WRITE, "shelf_member", "remove"),
    ("shelf", "remove_book"): (WRITE, "shelf_member", "remove"),
    ("shelf", "select"): (OBSERVATION, None, None),
    ("shelf", "open"): (OBSERVATION, None, None),
    ("wishlist", "add"): (WRITE, "wishlist", "add"),
    ("wishlist", "remove"): (WRITE, "wishlist", "remove"),
    ("wishlist", "delete"): (WRITE, "wishlist", "remove"),
    ("wishlist", "view"): (OBSERVATION, None, None),
    ("book", "add"): (WRITE, "book_add", None),
    ("book", "upload"): (WRITE, "book_add", None),
    ("book", "delete"): (OUT_OF_SCOPE, None, SCOPE_LIBRARY),
    ("book", "open"): (OBSERVATION, None, None),
    ("book", "view"): (OBSERVATION, None, None),
    ("book", "close"): (OBSERVATION, None, None),
    ("book", "detail"): (OBSERVATION, None, None),
    ("book", "browse"): (OBSERVATION, None, None),
    # `checkin` defaults to out-of-scope because confirming one writes a
    # physical copy; the two steps before that are only looks.
    ("checkin", "start"): (OBSERVATION, None, None),
    ("checkin", "lookup"): (OBSERVATION, None, None),
    ("checkin", "search"): (OBSERVATION, None, None),
}

# Names with no noun at all. `start.md` calls these out as special.
SINGLETONS = {
    "anomaly": (OBSERVATION, None, None),
    "note": (OBSERVATION, None, None),
    "flow": (OBSERVATION, None, None),
    "unmerge": (OUT_OF_SCOPE, None, SCOPE_LIBRARY),
}


@dataclass(frozen=True)
class Classification:
    """What the audit decided an action name means."""

    kind: str
    family: str | None = None
    detail: str | None = None
    reason: str | None = None

    @property
    def is_write(self) -> bool:
        return self.kind == WRITE


def normalise(action: str) -> tuple[str, ...]:
    """Lowercase an action name and split it into comparable segments."""
    return tuple(s for s in SPLIT.split(action.strip().lower()) if s)


def noun_of(action: str) -> str | None:
    """The noun an action name is about, aliases resolved."""
    segments = normalise(action)
    if not segments:
        return None
    return NOUN_ALIASES.get(segments[0], segments[0])


def _from_policy(noun: str, action: str) -> Classification:
    policy = NOUN_POLICY.get(noun)
    if policy == OBSERVATION:
        return Classification(OBSERVATION)
    if policy == OUT_OF_SCOPE:
        return Classification(OUT_OF_SCOPE, detail=SCOPE_FOR_NOUN.get(noun, SCOPE_LIBRARY))
    if policy == UNKNOWN:
        return Classification(
            UNKNOWN,
            reason=(
                f"unrecognised action {action!r}: {noun!r} is an audited noun, so the "
                "verb decides what to assert — add a (noun, verb) row to "
                "audit_lib/vocabulary.py"
            ),
        )
    return Classification(UNKNOWN, reason=f"unrecognised action {action!r} — {noun!r} is not a known noun")


def classify(action: str | None) -> Classification:
    """Resolve an action name to a `Classification`. Never raises."""
    if not action:
        return Classification(UNKNOWN, reason="entry has no action name")
    segments = normalise(action)
    if not segments:
        return Classification(UNKNOWN, reason=f"unparseable action {action!r}")

    # "I checked it stuck" — an observation on any noun, by rule.
    trimmed = list(segments)
    while len(trimmed) > 1 and trimmed[-1] in FILLER:
        trimmed.pop()
    if len(trimmed) > 1 and trimmed[-1] in QUALIFIERS:
        return Classification(OBSERVATION, reason=f"{action}: trailing {trimmed[-1]!r} reads as a look")

    compound = COMPOUND_NOUNS.get(tuple(trimmed[:2])) if len(trimmed) > 2 else None
    if compound:
        noun, trimmed = compound, [compound] + trimmed[2:]
    else:
        noun = NOUN_ALIASES.get(trimmed[0], trimmed[0])
    if len(trimmed) == 1:
        kind, family, detail = SINGLETONS.get(noun, (None, None, None))
        if kind is None:
            return _from_policy(noun, action)
        return Classification(kind, family, detail)

    verb = "_".join(trimmed[1:])
    exact = VERBS.get((noun, verb))
    if exact is not None:
        return Classification(*exact)

    # `book.detail.open` → `book.open`. Non-writes only: a write must match
    # `noun.verb` exactly, or a deeper name could assert the wrong direction
    # (`shelf.remove.book` is not `shelf.remove`).
    if len(trimmed) > 2:
        for candidate in (trimmed[-1], trimmed[1]):
            loose = VERBS.get((noun, candidate))
            if loose is not None and loose[0] != WRITE:
                return Classification(*loose)

    return _from_policy(noun, action)


def known_actions() -> list[str]:
    """Every `noun.verb` pair the table spells out, for `audit.py vocab`."""
    return sorted(f"{noun}.{verb}" for noun, verb in VERBS)
