"""Read the exploration settings out of the repo's gitignored `.env`.

`lib.sh` does this for the shell scripts; this is the same thing for the two
Python entry points, with the same precedence — an already-exported variable
wins, so a caller can point either tool at another instance without editing
`.env`. Only `OMNIBUS_EXPLORE_*` is read: the rest of that file is server
configuration and secrets these tools have no business loading.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

PREFIX = "OMNIBUS_EXPLORE_"


def repo_root(start: Path | None = None) -> Path | None:
    """The worktree root, or `None` when not in a git checkout."""
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=start or Path(__file__).resolve().parent,
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return Path(out.stdout.strip())


def parse(text: str) -> dict[str, str]:
    """Pull `OMNIBUS_EXPLORE_*` assignments out of a `.env` body.

    `.env` values are literal text, so surrounding quotes are stripped and
    `$VARS` / a leading `~` are expanded here — without that, the `$HOME/...`
    form `.env.example` recommends resolves to a directory that does not
    exist, and the journal silently lands somewhere nobody looks.
    """
    out: dict[str, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith(PREFIX) or "=" not in line:
            continue
        key, _, value = line.partition("=")
        value = value.strip()
        for quote in ('"', "'"):
            if len(value) >= 2 and value.startswith(quote) and value.endswith(quote):
                value = value[1:-1]
                break
        out[key.strip()] = os.path.expandvars(os.path.expanduser(value))
    return out


def load(env: dict[str, str] | None = None) -> None:
    """Export the repo's `OMNIBUS_EXPLORE_*` settings, without clobbering."""
    target = os.environ if env is None else env
    root = repo_root()
    if root is None:
        return
    env_file = root / ".env"
    if not env_file.is_file():
        return
    for key, value in parse(env_file.read_text(encoding="utf-8", errors="replace")).items():
        target.setdefault(key, value)
