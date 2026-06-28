# Running Omnibus with Docker

Omnibus ships a multi-stage [`Dockerfile`](../Dockerfile) and a
[`docker-compose.yml`](../docker-compose.yml) modelled on the Jellyfin
convention: bind-mount your media read-only, keep durable state in `/config`
and a regenerable cache in `/cache`, and configure everything through env.

> The Nix dev shell (see the [README](../README.md)) is still the supported way
> to *develop*. Docker is for *deploying* a built server.

## Quick start

```bash
# 1. Point the library mounts at your books and set your access URL.
$EDITOR docker-compose.yml

# 2. Build the bundle and start (first build is slow — it compiles the
#    workspace and the WASM client).
docker compose up -d --build

# 3. Open http://localhost:3000 (or whatever you set) and register.
#    The FIRST account created is automatically the admin.
```

## Volumes

| Container path | Contents | Back up? | Compose default |
|---|---|---|---|
| `/config` | SQLite DB (`omnibus.db`) + cover images | **Yes** | `./config` |
| `/cache` | WebP thumbnails + HLS transcode segments | No (regenerated) | `./cache` |
| `/books` | Ebook library | n/a (your data) | edit the `:ro` mount |
| `/audiobooks` | Audiobook library | n/a (your data) | edit the `:ro` mount |

Covers live under `/config` because they aren't reconstructible from the
library files; thumbnails and HLS segments live under `/cache` because the
server rebuilds them on demand and evicts them under a size cap.

## Key environment variables

| Variable | Why it matters |
|---|---|
| `OMNIBUS_PUBLIC_ORIGIN` | Must list the exact URL(s) you open in the browser, or authenticated POSTs are rejected with 403. Comma-separate multiples. |
| `OMNIBUS_SECURE_COOKIES` | Set `0` when serving plain `http://` (LAN/no TLS) — otherwise the session cookie is `Secure`-only and login silently fails. Set `1` behind HTTPS. |
| `EBOOK_LIBRARY_PATH` / `AUDIOBOOK_LIBRARY_PATH` | The in-container mount targets. Set both or neither (setting one clears the other). Omit both to configure libraries from the Settings UI instead. |
| `IP` / `PORT` | Bind address. The image defaults to `IP=0.0.0.0` so the container is reachable; `PORT` defaults to `3000`. |

The image bakes sensible defaults for `DATABASE_URL`, `OMNIBUS_COVERS_DIR`,
`OMNIBUS_THUMBS_DIR`, and `OMNIBUS_DATA_DIR` so they land in the volumes above —
override only if you change the mount layout. See [`.env.example`](../.env.example)
for the full annotated list of supported variables.

## File ownership (PUID / PGID)

Same convention as the linuxserver.io images: set `PUID` and `PGID` to your host
user's IDs (find them with `id -u` and `id -g`) so the server writes `./config`
and `./cache` as you, and files land owned by your account rather than root.
They default to `1000:1000`.

```yaml
environment:
  PUID: "1000"
  PGID: "1000"
```

The container starts as root only long enough for the entrypoint to apply these
IDs and fix ownership of the two volume roots, then drops to the unprivileged
`omnibus` user before running the server. The read-only library mounts just need
to be readable by that user. Migrating data that's currently owned by a
different UID? `chown` it once on the host — the entrypoint only adjusts the
mount roots, not their existing contents.

## Behind a reverse proxy (HTTPS)

Terminate TLS at nginx/Caddy/Traefik, proxy to the container's port, then:

- set `OMNIBUS_PUBLIC_ORIGIN` to your public `https://` origin,
- remove `OMNIBUS_SECURE_COOKIES` (or set `1`),
- only set `OMNIBUS_TRUST_FORWARDED_FOR=1` if the proxy strips inbound
  `X-Forwarded-For` — otherwise clients can spoof the rate-limit key. See the
  warning in [`.env.example`](../.env.example).

## Admin recovery

There is no separate admin-seed for production (the dev seed is compiled out of
release builds). If you lose admin access, set `OMNIBUS_INITIAL_ADMIN=<username>`
on an existing account, restart once, then **remove it** — it re-promotes on
every boot while set.

## Troubleshooting

- **Login does nothing / 403 on POST** — `OMNIBUS_PUBLIC_ORIGIN` doesn't match
  the browser URL, or `OMNIBUS_SECURE_COOKIES` is on over plain http.
- **Container unreachable** — confirm `IP=0.0.0.0` (the image default) and that
  the host port mapping isn't already taken.
- **No audiobook playback** — ffmpeg is bundled in the image; check the
  container logs for transcode errors and that the audiobook mount is populated.
- **Empty library** — verify the `:ro` mounts resolve to real directories on the
  host and that `EBOOK_LIBRARY_PATH` / `AUDIOBOOK_LIBRARY_PATH` match the mount
  targets.
