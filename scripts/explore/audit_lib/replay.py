"""Re-issue a journal suffix against the instance from its parameters alone.

A finding is an anecdote until someone can reproduce it, and the instance
accretes forever while the agents are non-deterministic — so replay is the
only way back to a suspicious state. It reuses `expectations.py`: the same
extraction that decides "the audit can check this" decides "the replayer can
redo this", which is why the two can never quietly disagree about what a
journal line meant.

Not everything is replayable, and the refusals are the honest part:

* **Wishlist add and check-in** carry an answer the server's ISBN lookup
  produced, not one the device held — replaying the recorded uuid would
  assert a binding this run never made.
* **Book upload** would add a second copy of the file rather than redo the
  first, and the source file may no longer exist.
* **Shelf membership** names its shelf by the display name in `params`, which
  is not a handle: two runs can leave two shelves with the same name.
* **A highlight without a machine location** cannot be placed — the server
  takes an `epub_cfi_range`, and prose ("chapter 4, para 12") is not one.

Each refusal names its reason, and says whether the journal or the replayer
is what falls short — a refusal that reads as "the journal was deficient"
when the capability was never built teaches agents the wrong lesson.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from .expectations import Expectation
from .state import ActorState

REFUSALS = {
    "wishlist": "wishlist adds carry the server's ISBN-lookup answer, not a payload the journal owns",
    "book_add": "re-uploading would add a second copy; replay it by hand from params.source_path",
    "shelf_member": "a shelf name is not a handle — two runs can leave two shelves with one name",
}


@dataclass
class Step:
    """One replay attempt and what the instance said."""

    actor: str
    seq: int | None
    family: str
    target: str | None
    action: str
    status: int | None
    detail: str

    @property
    def ok(self) -> bool:
        return self.status is not None and 200 <= self.status < 300


def _refuse(exp: Expectation, why: str) -> Step:
    return Step(exp.actor, exp.seq, exp.family, exp.target, "refused", None, why)


def replay_one(exp: Expectation, state: ActorState) -> Step:
    """Re-issue one expectation's write. Never raises on an HTTP failure."""
    refusal = REFUSALS.get(exp.family)
    if refusal:
        return _refuse(exp, refusal)

    client = state.client
    uuid = exp.target or ""

    if exp.family == "rating":
        if exp.value is None:
            status, body = client.post("/api/rpc/ratings/clear", {"uuid": uuid})
            return Step(exp.actor, exp.seq, exp.family, uuid, "clear rating", status, body[:160])
        status, body = client.post("/api/ratings", {"book_uuid": uuid, "stars": float(exp.value)})
        return Step(exp.actor, exp.seq, exp.family, uuid, f"set rating {exp.value:g}", status, body[:160])

    if exp.family == "read_status":
        status, body = client.put("/api/read-status", {"book_uuid": uuid, "status": exp.value})
        return Step(exp.actor, exp.seq, exp.family, uuid, f"set status {exp.value}", status, body[:160])

    if exp.family == "playback_rate":
        # The server function takes two arguments — the uuid and the update
        # struct — and the struct's field is `playback_rate`, range 0.5..=3.
        status, body = client.post(
            "/api/rpc/audiobooks/playback-rate/set",
            {"uuid": uuid, "update": {"playback_rate": float(exp.value)}},
        )
        return Step(exp.actor, exp.seq, exp.family, uuid, f"set rate {exp.value:g}x", status, body[:160])

    if exp.family == "progress":
        payload = _progress_payload(uuid, exp.value or {})
        if payload is None:
            return _refuse(exp, "params record no position on the format's own axis")
        status, body = client.post("/api/progress", payload)
        return Step(exp.actor, exp.seq, exp.family, uuid, f"save {payload['format']} position", status, body[:160])

    if exp.family == "journal":
        status, body = client.post("/api/journals", {"book_uuid": uuid, "body_md": str(exp.value)})
        return Step(exp.actor, exp.seq, exp.family, uuid, "create journal entry", status, body[:160])

    if exp.family == "highlight":
        keys = exp.value if isinstance(exp.value, dict) else {}
        cfi = keys.get("cfi") or exp.extra.get("cfi")
        if not cfi:
            return _refuse(
                exp,
                "params carry no epubcfi range — a highlight is placed at the machine "
                "location, and prose cannot stand in for it",
            )
        payload: dict[str, Any] = {
            "book_uuid": uuid,
            "epub_cfi_range": cfi,
            "color": str(keys.get("colour") or "amber").lower(),
        }
        if keys.get("quote"):
            payload["text"] = keys["quote"]
        status, body = client.post("/api/highlights", payload)
        note = keys.get("note")
        if note and 200 <= status < 300:
            # `CreateHighlight` has no note field, and unknown fields are
            # silently dropped — sending it inline would 200 and lose the
            # note. It lands via its own PATCH on the created row.
            try:
                highlight_id = json.loads(body).get("id")
            except (json.JSONDecodeError, AttributeError):
                highlight_id = None
            if highlight_id is None:
                return Step(
                    exp.actor, exp.seq, exp.family, uuid,
                    "create highlight (note NOT set — no id in the create response)",
                    status, body[:160],
                )
            status, body = client.patch(f"/api/highlights/{highlight_id}/note", {"note": note})
            return Step(exp.actor, exp.seq, exp.family, uuid, "create highlight + note", status, body[:160])
        return Step(exp.actor, exp.seq, exp.family, uuid, "create highlight", status, body[:160])

    if exp.family == "bookmark":
        keys = exp.value if isinstance(exp.value, dict) else {}
        position = keys.get("position") or exp.extra.get("position")
        if not position:
            return _refuse(exp, "params carry no position — a bookmark cannot be placed without one")
        payload = {"book_uuid": uuid, "position": position}
        if keys.get("label"):
            payload["title"] = keys["label"]
        status, body = client.post("/api/bookmarks", payload)
        return Step(exp.actor, exp.seq, exp.family, uuid, "create bookmark", status, body[:160])

    if exp.family == "shelf":
        status, body = client.post(
            "/api/shelves",
            {"kind": "manual", "name": str(exp.value), "visibility": "private", "rules": [], "book_uuids": []},
        )
        return Step(exp.actor, exp.seq, exp.family, None, f"create shelf {exp.value!r}", status, body[:160])

    return _refuse(exp, f"no replay rule for family {exp.family!r}")


def _progress_payload(uuid: str, position: dict[str, Any]) -> dict[str, Any] | None:
    """Build a `ProgressUpdate`; `None` when the axis has no recorded value.

    The server validates that `epub` carries a cfi and `audio` carries an
    offset, so a payload missing its own axis is a 400 rather than a replay.
    """
    if position.get("axis") == "audio":
        if position.get("seconds") is None:
            return None
        return {
            "book_uuid": uuid,
            "format": "audio",
            "audio_position_seconds": float(position["seconds"]),
        }
    cfi = position.get("cfi")
    if not cfi:
        return None
    payload = {"book_uuid": uuid, "format": "epub", "epub_cfi": cfi}
    if position.get("percent") is not None:
        payload["progress_percent"] = int(position["percent"])
    return payload
