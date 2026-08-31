# Omnibus Helm chart

Deploys [Omnibus](https://github.com/seamus-sloan/omnibus) — a self-hosted
ebook/audiobook library — onto Kubernetes. It wraps the same
`sesloan/omnibus` image the [Docker guide](../../docs/docker.md) uses.

```bash
helm install omnibus ./charts/omnibus \
  --namespace omnibus --create-namespace \
  --set ingress.enabled=true \
  --set ingress.hosts[0].host=books.example.com \
  --set ingress.hosts[0].paths[0].path=/ \
  --set ingress.hosts[0].paths[0].pathType=Prefix \
  --set ingress.tls[0].secretName=omnibus-tls \
  --set ingress.tls[0].hosts[0]=books.example.com \
  --set libraries.ebooks.enabled=true \
  --set libraries.ebooks.volume.persistentVolumeClaim.claimName=my-ebooks
```

The **first account you register becomes the admin**, so register before
exposing the instance. Then `helm test omnibus -n omnibus` to confirm the
health endpoint answers.

## Design decisions

Most of this chart follows from facts about the app rather than from Helm
convention. The non-obvious ones:

### Why one replica

Omnibus keeps everything — library metadata, per-reader progress, highlights,
shelves — in one SQLite file on the `/config` volume. Two pods writing it
concurrently corrupts it, so `replicas` is **hard-coded to 1 and not exposed as
a value**, and the update strategy is `Recreate` rather than `RollingUpdate`:
a rolling update would briefly run both pods against the same volume, which is
the exact thing the single replica prevents. The cost is a few seconds of
downtime on every upgrade.

For the same reason the PVCs default to `ReadWriteOnce`. Putting the config
volume on RWX shared storage (NFS, CephFS) does not enable scaling — it just
removes the guard rail.

### Volume split, and one fix to the image's defaults

| Mount | Contents | Back up |
|---|---|---|
| `/config` | SQLite DB, covers, **journal images** | **Yes** |
| `/cache` | thumbnails, HLS segments, KEPUB/converted output, export EPUBs, logs | No |
| library mounts | your books | your data |

The image defaults `OMNIBUS_DATA_DIR=/cache/data` and leaves
`OMNIBUS_JOURNAL_IMAGES_DIR` unset — so images embedded in journal entries land
under `/cache`, which every doc (correctly) describes as safe to delete.
`.env.example` says the opposite about that directory: *"Durable user data —
NOT a regenerable cache — so back it up with the DB."* The chart resolves the
contradiction by pinning `OMNIBUS_JOURNAL_IMAGES_DIR=/config/journal-images`.

Worth fixing in `Dockerfile`/`docker-compose.yml` too — the same trap exists
for Compose users today.

### Libraries are passthrough, never provisioned

Your books already exist somewhere. Each entry under `libraries` takes a **raw
Kubernetes volume source** spliced into the pod as-is, so PVC, NFS, hostPath,
and CSI all work without the chart enumerating them:

```yaml
libraries:
  ebooks:
    enabled: true
    readOnly: false      # in-app uploads write here
    volume:
      persistentVolumeClaim:
        claimName: my-ebooks
  audiobooks:
    enabled: true
    readOnly: true
    volume:
      nfs: { server: 10.0.0.5, path: /export/audiobooks }
```

`volume` ships **empty**: Helm deep-merges values, so a default source in
`values.yaml` would merge with yours instead of being replaced, producing a
volume with two sources that the API server rejects. Enabling a library without
a volume fails the render with a message saying so.

`libraryPathsFromEnv` publishes the mount paths as `EBOOK_LIBRARY_PATH` /
`AUDIOBOOK_LIBRARY_PATH`. The server re-seeds those into its settings row on
**every** boot, and the hook writes *both* rows whenever either variable is
present — so a chart that set only one would silently clear the other. Set it
`false` to manage library paths from the Settings UI instead.

### The access URL is the #1 footgun

`OMNIBUS_PUBLIC_ORIGIN` is a CSRF allowlist. When it doesn't match the URL in
the browser, reads keep working and every write returns 403 — a failure that
looks like a broken app, not a config error. The chart derives it from
`ingress.hosts`, picking each host's scheme from whether **that host** appears
in a TLS block, so a mixed-TLS ingress gets `https://a,http://b` rather than
one scheme applied to both.

`OMNIBUS_SECURE_COOKIES` follows from the same derivation, because a Secure
cookie is never sent over `http://` and login silently fails to stick. If you
mix TLS and plaintext hosts, one of them is broken either way — `NOTES.txt`
warns and names the hosts.

Override both with `publicOrigin` / `secureCookies` for LoadBalancer or
NodePort installs, where the chart has no host to derive from.

### Two security modes

The image's entrypoint starts as **root**, remaps its `omnibus` user to
`PUID`/`PGID`, chowns the volume roots, and drops privileges with `gosu`.
Kubernetes does that job with `fsGroup`, so by default the chart **bypasses the
entrypoint** (`command: ["/app/server"]`) and runs unprivileged, non-root,
read-only-rootfs, all capabilities dropped — which satisfies the `restricted`
Pod Security Standard.

Set `puid.enabled=true` to use the image's entrypoint instead. Needed when the
volumes are **NFS-backed**, where `fsGroup` has no effect and ownership must
match a real uid/gid on the server. The chart then adjusts the container
accordingly — root, `readOnlyRootFilesystem: false` (usermod rewrites
`/etc/passwd`), and just the `CHOWN`/`DAC_OVERRIDE`/`FOWNER`/`SETGID`/`SETUID`
capabilities.

`fsGroupChangePolicy: OnRootMismatch` keeps a large cover directory from being
recursively relabelled on every pod start.

### Probes

All three hit `GET /api/_health`, which is unauthenticated by design
(`auth::gate` whitelists it alongside `/api/auth/*`). The startup probe allows
5 minutes: migrations and the `_norm` backfills run before the first response,
and a cold, large database is slow. Liveness is deliberately slack — killing a
pod mid-transcode throws the work away.

### Ingress tuning

With `ingress.tuning.enabled` the chart derives three ingress-nginx annotations
you'd otherwise discover the hard way:

- `proxy-body-size` from `config.maxUploadBytes` — nginx's 1 MB default rejects
  book uploads with a 413 before the app ever sees them.
- `proxy-read-timeout` / `proxy-send-timeout` — audiobook streams and large
  downloads outlive the 60 s default.
- a `server-snippet` denying `/metrics`. Only `/api/*` is behind the auth gate,
  so the Prometheus endpoint is **publicly readable** if the ingress routes `/`.
  (Some ingress-nginx installs set `allow-snippet-annotations: false`, which
  drops this silently — check, or block `/metrics` another way.)

Anything you put in `ingress.annotations` wins over the derived value.

### Rate limiting behind a proxy

`config.trustForwardedFor` maps to `OMNIBUS_TRUST_FORWARDED_FOR`, which lets
the login throttle key on `X-Forwarded-For`. It defaults **off**: on a directly
reachable Service, any client can spoof the header for a fresh bucket.

Note the app consults the header only when it has no direct peer address
(`client_ip` in `server/src/rate_limit.rs` prefers `ConnectInfo`). Behind an
ingress the peer address is the controller pod, so depending on whether the
serving stack supplies `ConnectInfo`, the throttle may end up keyed on the
controller's IP — one shared bucket for every user. Worth verifying against
your own ingress before relying on per-IP limits.

## Operations

**Back up `/config`.** Nothing else in the release is recoverable. The config
PVC carries `helm.sh/resource-policy: keep` so `helm uninstall` won't delete it
(`persistence.config.retain=false` to opt out). Because SQLite is live, prefer a
volume snapshot or a `sqlite3 .backup` against a stopped pod over a raw file
copy.

**Upgrades** stop the old pod before starting the new one. Migrations run at
boot from `init_db`, are forward-only, and are checksummed — so roll *forward*
after a failed upgrade; restoring an older image against a migrated database
will not work.

**Admin recovery**: set `config.initialAdmin=<username>`, upgrade, log in, then
clear it and upgrade again. It re-promotes on every boot while set, and
`NOTES.txt` reminds you.

## Values

See [`values.yaml`](values.yaml) — every key is commented in place. The ones
you will actually set:

| Key | Default | Notes |
|---|---|---|
| `libraries.{ebooks,audiobooks}.enabled` | `false` | Off so the chart renders bare; enable what you have |
| `libraries.*.volume` | `{}` | Raw volume source; required when enabled |
| `publicOrigin` | derived from ingress | Required for LoadBalancer/NodePort |
| `secureCookies` | derived from TLS | Force `"false"` for plain-http LAN installs |
| `persistence.config.size` | `10Gi` | DB + covers + journal images |
| `persistence.cache.size` | `20Gi` | Keep above the sum of the cache caps |
| `puid.enabled` | `false` | Turn on for NFS-backed volumes |
| `ingress.tuning.enabled` | `true` | ingress-nginx annotations |
| `serviceMonitor.enabled` | `false` | Needs the Prometheus operator CRD |
| `secrets.*` | `""` | Hardcover / Google Books keys, SMTP; or `existingSecret` |

## Not covered

- **No autoscaling / PDB / HPA.** Meaningless at one replica.
- **No backup CronJob.** Use your cluster's volume snapshots.
- **Not published to a chart repo** yet. The repo already publishes `gh-pages`
  for the marketing site, so `chart-releaser` could serve it from the same
  branch if that's wanted.
