"""A small authenticated HTTP client for the exploration instance.

Per-user state is only readable *as* that user — there is no admin route that
returns another account's ratings or progress — so the audit logs in once per
actor with the credentials the run already minted. It uses `urllib` rather
than `requests` because the exploration scripts assume nothing beyond the
system Python that `lib.sh` already relies on.
"""

from __future__ import annotations

import http.cookiejar
import json
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

TIMEOUT = 30

# `/api/auth/*` is rate-limited to 10 requests per 60s per IP
# (server::rate_limit::MAX_REQUESTS). One audit does one login per actor, so a
# swarm past ten agents — or two audits back to back — trips it. A 429 on
# login is a wait, not a failure: giving up here would report a whole actor's
# state as unreadable.
AUTH_WINDOW_SECS = 60
LOGIN_RETRIES = 3

# Distinguishes "no body" from "a body that is literally null".
_NO_BODY = object()


class ApiError(Exception):
    """A request to the instance failed."""


@dataclass(frozen=True)
class Account:
    """One exploration account, as `provision.sh` emits it."""

    actor: str
    username: str
    password: str


def load_accounts(raw: Any) -> dict[str, Account]:
    """Parse `provision.sh`'s JSON into actor-keyed accounts."""
    if not isinstance(raw, list):
        raise ApiError("accounts file must be the JSON array provision.sh emits")
    out: dict[str, Account] = {}
    for item in raw:
        if not isinstance(item, dict) or not all(k in item for k in ("actor", "username", "password")):
            raise ApiError(f"account entry missing actor/username/password: {item!r}")
        out[str(item["actor"])] = Account(str(item["actor"]), str(item["username"]), str(item["password"]))
    return out


class Client:
    """A logged-in session against one instance."""

    def __init__(self, base_url: str) -> None:
        self.base = base_url.rstrip("/")
        self._jar = http.cookiejar.CookieJar()
        self._opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self._jar))
        self.username: str | None = None
        self.user_id: int | None = None

    def _request(self, path: str, body: Any = _NO_BODY, method: str | None = None) -> tuple[int, str]:
        has_body = body is not _NO_BODY
        data = json.dumps(body).encode("utf-8") if has_body else None
        req = urllib.request.Request(self.base + path, data=data, method=method or ("POST" if has_body else "GET"))
        # Every mutating request needs an allowed Origin or auth::origin_check
        # 403s it regardless of session; harmless on reads, so it is uniform.
        req.add_header("Origin", self.base)
        if has_body:
            req.add_header("Content-Type", "application/json")
        try:
            with self._opener.open(req, timeout=TIMEOUT) as resp:
                return resp.status, resp.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            return exc.code, exc.read().decode("utf-8", "replace")
        except OSError as exc:
            raise ApiError(f"{method or 'GET'} {path}: {exc}") from exc

    def get_json(self, path: str, default: Any = None) -> Any:
        """GET a JSON body. A 404 yields `default`; other non-200s raise."""
        status, text = self._request(path)
        if status == 404:
            return default
        if status != 200:
            raise ApiError(f"GET {path} -> HTTP {status}: {text[:200]}")
        return json.loads(text) if text.strip() else default

    def rpc(self, path: str, payload: dict[str, Any], default: Any = None) -> Any:
        """Call a Dioxus server function. Its arguments are a named object."""
        status, text = self._request(path, payload)
        if status != 200:
            raise ApiError(f"POST {path} -> HTTP {status}: {text[:200]}")
        return json.loads(text) if text.strip() else default

    def post(self, path: str, payload: Any) -> tuple[int, str]:
        """Raw POST, for the replayer — the caller judges the status."""
        return self._request(path, payload)

    def put(self, path: str, payload: Any) -> tuple[int, str]:
        return self._request(path, payload, method="PUT")

    def login(self, username: str, password: str, sleep=time.sleep) -> None:
        """Authenticate, failing loudly rather than reading as an empty account."""
        body = {"username": username, "password": password}
        for attempt in range(LOGIN_RETRIES):
            status, text = self._request("/api/auth/login", body)
            if status != 429:
                break
            if attempt < LOGIN_RETRIES - 1:
                sleep(AUTH_WINDOW_SECS + 2)
        if status != 200:
            raise ApiError(f"login as {username} failed (HTTP {status}): {text[:200]}")
        me = self.get_json("/api/auth/me") or {}
        self.username = me.get("username")
        self.user_id = me.get("id")
        if self.username is None:
            raise ApiError(f"login as {username} returned no identity — session did not stick")


def health(base_url: str) -> int:
    """HTTP status of the instance's health endpoint."""
    client = Client(base_url)
    status, _ = client._request("/api/_health")
    return status
