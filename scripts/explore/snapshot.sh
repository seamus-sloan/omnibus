#!/usr/bin/env bash
# Snapshot / restore the exploration instance's durable state.
#
# Usage:
#   snapshot.sh take [label]     capture a snapshot (prints its name)
#   snapshot.sh list             list snapshots, newest first
#   snapshot.sh restore <name>   restore one, replacing current state
#
# The instance database accretes across runs and is never reset, so a snapshot
# taken before each run is the only way back from a run that corrupts something
# — and it doubles as the baseline the intent-vs-state audit diffs against.
#
# Why a tarball and not an LVM snapshot: the applications VM's volume group has
# zero free extents, so `lvcreate --snapshot` cannot allocate. The instance's
# whole durable footprint is a few MB (a SQLite DB, covers, and the library),
# which tars in about a second and stays quick well past a few hundred MB.
#
# The container is stopped for the duration of the copy. SQLite runs in WAL
# mode, so copying the DB from underneath a live writer can capture a torn
# pair of .db and .db-wal; stopping is a couple of seconds on a test instance
# and removes the question entirely.

set -euo pipefail

HOST="${OMNIBUS_EXPLORE_SSH_HOST:-applications}"
DIR="${OMNIBUS_EXPLORE_REMOTE_DIR:-omnibus-main}"
SNAPS="${OMNIBUS_EXPLORE_SNAPSHOT_DIR:-omnibus-snapshots}"
KEEP="${OMNIBUS_EXPLORE_SNAPSHOT_KEEP:-10}"

cmd="${1:?usage: snapshot.sh take [label] | list | restore <name>}"

case "$cmd" in
  take)
    label="${2:-run}"
    name="$(date -u +%Y%m%dT%H%M%SZ)-${label//[^A-Za-z0-9_-]/_}"
    ssh "$HOST" "set -euo pipefail
      mkdir -p ~/$SNAPS
      cd ~/$DIR
      docker compose stop >/dev/null 2>&1
      tar czf ~/$SNAPS/$name.tgz config books audiobooks
      docker compose start >/dev/null 2>&1
      # Keep only the newest \$KEEP; a snapshot nobody can restore from is just disk.
      ls -1t ~/$SNAPS/*.tgz 2>/dev/null | tail -n +$((KEEP + 1)) | xargs -r rm -f
      du -h ~/$SNAPS/$name.tgz | cut -f1" \
      | sed "s|^|snapshot $name  |"
    printf '%s\n' "$name"
    ;;

  list)
    ssh "$HOST" "ls -1t ~/$SNAPS/*.tgz 2>/dev/null | while read -r f; do
        printf '%s  %s\n' \"\$(du -h \"\$f\" | cut -f1)\" \"\$(basename \"\$f\" .tgz)\"
      done" || echo "(none)"
    ;;

  restore)
    name="${2:?usage: snapshot.sh restore <name>}"
    ssh "$HOST" "set -euo pipefail
      [ -f ~/$SNAPS/$name.tgz ] || { echo 'no such snapshot: $name' >&2; exit 1; }
      cd ~/$DIR
      docker compose down >/dev/null 2>&1
      rm -rf config books audiobooks
      tar xzf ~/$SNAPS/$name.tgz
      docker compose up -d >/dev/null 2>&1"
    echo "restored $name — waiting for health"
    for _ in $(seq 1 20); do
      s=$(ssh "$HOST" "docker inspect -f '{{.State.Health.Status}}' omnibus-main 2>/dev/null" || true)
      [ "$s" = healthy ] && break
      sleep 3
    done
    echo "health=${s:-unknown}"
    ;;

  *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
