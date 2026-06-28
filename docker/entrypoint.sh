#!/bin/sh
# PUID/PGID entrypoint, linuxserver.io-style. The container starts as root so
# it can remap the unprivileged `omnibus` user to the host UID/GID the operator
# wants, fix ownership of the writable volumes, then drop privileges before
# exec'ing the server. Defaults to 1000:1000 when PUID/PGID are unset.
set -e

PUID="${PUID:-1000}"
PGID="${PGID:-1000}"

# Remap the baked-in omnibus user/group to the requested ids. `-o` allows
# non-unique ids (e.g. sharing 1000 with another account) and these are no-ops
# when the ids already match.
groupmod -o -g "$PGID" omnibus
usermod  -o -u "$PUID" omnibus

# Own only the volume mount roots, not their (potentially huge) contents — the
# server runs as omnibus and creates covers/thumbs/data files itself. Migrating
# pre-existing data owned by a different uid? chown it once on the host.
chown omnibus:omnibus /config /cache 2>/dev/null || true

echo "omnibus: starting as uid=${PUID} gid=${PGID}"
exec gosu omnibus:omnibus "$@"
