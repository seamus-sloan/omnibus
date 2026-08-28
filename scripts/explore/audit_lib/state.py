"""Read the per-user state the audit reconciles the journal against.

Everything here is a *projection*: the small set of facts an audit finding can
be phrased in terms of, not a mirror of the database. It is deliberately
narrow — progress, read status, ratings, playback rate, journal entries,
highlights, bookmarks, shelves and wishlist — because those are the writes
`start.md` asks an agent to journal. Metadata overrides, genres and covers are
excluded by design: they are free-for-all edits, so no single intent owns the
final value.

Reads are cached per `(actor, book)` because a run journals many entries
against the same handful of books, and an audit that re-fetched per entry
would make more requests than the run did.
"""

from __future__ import annotations

from typing import Any

from .client import Account, ApiError, Client

# Reads split across two surfaces for the same reason the app does: the REST
# router carries what mobile needs, and the rest are Dioxus server functions.
RPC_RATING = "/api/rpc/ratings/get"
RPC_RATE = "/api/rpc/audiobooks/playback-rate/get"
RPC_JOURNALS = "/api/rpc/journals/list"
RPC_HIGHLIGHTS = "/api/rpc/highlights/list"
RPC_BOOKMARKS = "/api/rpc/bookmarks/list"


class ActorState:
    """Live per-user state for one actor, read lazily and cached."""

    def __init__(self, client: Client, actor: str) -> None:
        self.client = client
        self.actor = actor
        self._cache: dict[tuple[str, str], Any] = {}

    def _memo(self, kind: str, key: str, fetch) -> Any:
        slot = (kind, key)
        if slot not in self._cache:
            self._cache[slot] = fetch()
        return self._cache[slot]

    # --- scalars ----------------------------------------------------------
    def rating(self, uuid: str) -> float | None:
        rec = self._memo("rating", uuid, lambda: self.client.rpc(RPC_RATING, {"uuid": uuid}))
        return None if not rec else rec.get("stars")

    def read_status(self, uuid: str) -> str | None:
        rec = self._memo("status", uuid, lambda: self.client.get_json(f"/api/read-status/{uuid}"))
        return None if not rec else rec.get("status")

    def progress(self, uuid: str, axis: str = "ebook") -> dict[str, Any] | None:
        """The saved position on one axis.

        `?format=` is not decoration: the route defaults to `epub`, and a
        dual-format book keeps a separate row per format — so asking without
        it reports a listener's position as missing.
        """
        fmt = "audio" if axis == "audio" else "epub"
        return self._memo(
            "progress", f"{uuid}:{fmt}", lambda: self.client.get_json(f"/api/progress/{uuid}?format={fmt}")
        )

    def playback_rate(self, uuid: str) -> float | None:
        rec = self._memo("rate", uuid, lambda: self.client.rpc(RPC_RATE, {"uuid": uuid}))
        return None if not rec else rec.get("playback_rate")

    # --- lists ------------------------------------------------------------
    def journals(self, uuid: str) -> list[dict[str, Any]]:
        """This actor's own journal entries on a book.

        The endpoint is book-scoped and returns every reader's entries, so the
        author filter is what keeps one agent's audit from passing on another
        agent's writing.
        """
        rows = self._memo("journals", uuid, lambda: self.client.rpc(RPC_JOURNALS, {"book_uuid": uuid}) or [])
        return [r for r in rows if self.client.user_id is None or r.get("author_id") == self.client.user_id]

    def highlights(self, uuid: str) -> list[dict[str, Any]]:
        return self._memo("highlights", uuid, lambda: self.client.rpc(RPC_HIGHLIGHTS, {"book_uuid": uuid}) or [])

    def bookmarks(self, uuid: str) -> list[dict[str, Any]]:
        return self._memo("bookmarks", uuid, lambda: self.client.rpc(RPC_BOOKMARKS, {"book_uuid": uuid}) or [])

    def shelves(self) -> list[dict[str, Any]]:
        """Shelves owned by this actor, wishlist included."""
        rows = self._memo("shelves", "", lambda: self.client.get_json("/api/shelves", []) or [])
        return [s for s in rows if self.client.user_id is None or s.get("owner_user_id") == self.client.user_id]

    def shelf_members(self, shelf_id: int) -> list[str]:
        page = self._memo("shelf", str(shelf_id), lambda: self.client.get_json(f"/api/shelves/{shelf_id}/page", {}))
        return _uuids_in(page)

    def wishlist(self) -> list[str]:
        """Book uuids on this actor's wishlist shelf."""
        for shelf in self.shelves():
            if shelf.get("kind") == "wishlist":
                return self.shelf_members(int(shelf["id"]))
        return []

    def library(self) -> list[str]:
        """Every book uuid in the library — shared, not per-user."""
        return self._memo("library", "", lambda: _uuids_in(self.client.get_json("/api/ebooks", {})))


def _uuids_in(payload: Any) -> list[str]:
    """Pull book uuids out of a library or shelf-page response.

    The listing wraps its rows in `{path, books, error, total}` and names the
    uuid `unique_identifier`; a shelf page has its own envelope. Reading both
    shapes here keeps that spelling in one place.
    """
    if payload is None:
        return []
    rows = payload
    if isinstance(payload, dict):
        for key in ("books", "items", "members", "entries"):
            if isinstance(payload.get(key), list):
                rows = payload[key]
                break
        else:
            rows = []
    if not isinstance(rows, list):
        return []
    out = []
    for row in rows:
        if isinstance(row, str):
            out.append(row)
        elif isinstance(row, dict):
            uuid = row.get("unique_identifier") or row.get("uuid") or row.get("book_uuid")
            if uuid:
                out.append(str(uuid))
    return out


def open_actor(base_url: str, account: Account) -> ActorState:
    """Log in as one actor and hand back its state reader."""
    client = Client(base_url)
    client.login(account.username, account.password)
    return ActorState(client, account.actor)


def capture(base_url: str, accounts: dict[str, Account], max_books: int = 200) -> dict[str, Any]:
    """Snapshot every audited fact, as the baseline a later run diffs against.

    Called before a run, when nothing yet says which books will be touched, so
    it sweeps the whole library for each actor. `max_books` is a guard rail:
    the sweep is O(books × actors) requests and a library that has grown past
    a couple of hundred wants a narrower baseline, not a slower audit.
    """
    out: dict[str, Any] = {"base_url": base_url.rstrip("/"), "actors": {}}
    library: list[str] = []
    for actor, account in sorted(accounts.items()):
        state = open_actor(base_url, account)
        if not library:
            library = state.library()
            out["library"] = library
        if len(library) > max_books:
            raise ApiError(
                f"library holds {len(library)} books, over the {max_books} baseline cap — "
                "raise --max-books deliberately or narrow the baseline"
            )
        per_book: dict[str, Any] = {}
        for uuid in library:
            per_book[uuid] = {
                "rating": state.rating(uuid),
                "read_status": state.read_status(uuid),
                "progress": _thin_progress(state.progress(uuid, "ebook")),
                "progress_audio": _thin_progress(state.progress(uuid, "audio")),
                "playback_rate": state.playback_rate(uuid),
                "journals": [_thin_journal(j) for j in state.journals(uuid)],
                "highlights": [_thin_annotation(h) for h in state.highlights(uuid)],
                "bookmarks": [_thin_annotation(b) for b in state.bookmarks(uuid)],
            }
        out["actors"][actor] = {
            "username": account.username,
            "user_id": state.client.user_id,
            "books": per_book,
            "shelves": [s.get("name") for s in state.shelves()],
            "wishlist": state.wishlist(),
        }
    return out


def _thin_progress(rec: dict[str, Any] | None) -> dict[str, Any] | None:
    if not rec:
        return None
    return {k: rec.get(k) for k in ("format", "epub_cfi", "audio_position_seconds", "progress_percent")}


def _thin_journal(rec: dict[str, Any]) -> dict[str, Any]:
    return {"id": rec.get("id"), "body_md": rec.get("body_md")}


def _thin_annotation(rec: dict[str, Any]) -> dict[str, Any]:
    return {k: rec.get(k) for k in ("id", "note", "text", "color", "label", "cfi") if k in rec}
